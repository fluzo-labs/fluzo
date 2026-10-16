//! RUN-05 admission integrated with the strict loopback HTTP simulator.
//!
//! Every request in these tests passes through the
//! `AdmissionController` before a socket is used, and the strict
//! simulator fails the run on any request it did not script. Together
//! they evidence PRD 43.2/43.3: pool limits serialize real traffic,
//! a 429 produces a typed rate-limit outcome with zero automatic
//! retry, a redirect is never followed, and a timed-out queue wait
//! sends nothing.

// The shared support module is written for the full simulator suite;
// this binary uses a subset, so its unused items are not dead code.
#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;

use fluzo_core::admission::{ProviderRejection, RequestClass, classify_status};
use fluzo_runtime::admission::{
    Admission, AdmissionController, AdmissionDenied, ModelId, PoolId, PoolLimits, ReleaseKind,
};
use hyper::StatusCode;
use serde_json::json;
use support::{Chunk, Failure, Fixture, Reply, Scenario, Step, collect};

fn controller(max: u32) -> AdmissionController {
    let controller = AdmissionController::new();
    let mut models = BTreeMap::new();
    models.insert(ModelId("coder".to_owned()), max);
    controller
        .register_pool(
            PoolId("shared".to_owned()),
            PoolLimits::new(max, 1, 8).unwrap(),
            30_000,
            models,
        )
        .unwrap();
    controller
}

fn chat_body(n: u64) -> Vec<u8> {
    json!({"model": "coder", "messages": [{"role": "user", "content": n.to_string()}]})
        .to_string()
        .into_bytes()
}

fn step_reply(n: u64) -> Reply {
    Reply::bytes(
        StatusCode::OK,
        json!({"choices": [{"message": {"role": "assistant", "content": format!("ok-{n}")}}]})
            .to_string(),
    )
}

fn chat_step(n: u64) -> Step {
    Step {
        method: "POST".to_owned(),
        path: "/v1/chat/completions".to_owned(),
        headers: vec![],
        body: Some(
            json!({"model": "coder", "messages": [{"role": "user", "content": n.to_string()}]}),
        ),
        reply: step_reply(n),
    }
}

fn admitted_slot(
    controller: &AdmissionController,
    now_ms: u64,
) -> fluzo_runtime::admission::SlotId {
    match controller.try_admit(
        &fluzo_runtime::admission::AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId("shared".to_owned()),
            model: ModelId("coder".to_owned()),
            input_estimate_tokens: 100,
            max_response_output_tokens: 50,
            remaining_task_seconds: 3600,
            budgets: None,
            consumed: None,
        },
        now_ms,
    ) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    }
}

#[tokio::test]
async fn pool_limit_serializes_real_requests_through_the_simulator() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(1), chat_step(2)],
    })
    .await
    .unwrap();
    let controller = controller(1);
    let endpoint = fixture.endpoint();

    // First request is admitted; the second must queue behind it.
    let slot_one = admitted_slot(&controller, 0);
    let queued = controller.try_admit(
        &fluzo_runtime::admission::AdmissionRequest {
            class: RequestClass::Compaction,
            pool: PoolId("shared".to_owned()),
            model: ModelId("coder".to_owned()),
            input_estimate_tokens: 100,
            max_response_output_tokens: 50,
            remaining_task_seconds: 3600,
            budgets: None,
            consumed: None,
        },
        0,
    );
    assert!(matches!(queued, Admission::Queued { position: 1, .. }));

    // The fixture serves one request per connection (keep-alive off),
    // so each admitted request opens its own connection.
    let mut client_one = fixture.connect(&endpoint).await.unwrap();
    let response = client_one
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    collect(response).await.unwrap();
    drop(client_one);
    controller.release(slot_one, ReleaseKind::Completed);

    let (ticket_two, slot_two) = controller
        .admit_next(&PoolId("shared".to_owned()), 10)
        .expect("freed slot goes to the queued request");
    let mut client_two = fixture.connect(&endpoint).await.unwrap();
    let response = client_two
        .request("POST", "/v1/chat/completions", chat_body(2).into())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    collect(response).await.unwrap();
    controller.release(slot_two, ReleaseKind::Completed);
    drop(client_two);

    let report = fixture.finish().await;
    assert!(
        report.failures.is_empty(),
        "strict simulator failures: {report:?}"
    );
    assert_eq!(report.requests, 2);
    assert_eq!(report.consumed, 2);
    assert!(ticket_two.0 > 0, "the dispatched waiter carries its ticket");
}

#[tokio::test]
async fn rate_limited_429_is_never_retried() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![Step {
            method: "POST".to_owned(),
            path: "/v1/chat/completions".to_owned(),
            headers: vec![],
            body: Some(json!({"model": "coder", "messages": [{"role": "user", "content": "1"}]})),
            reply: Reply {
                status: StatusCode::TOO_MANY_REQUESTS,
                headers: vec![("retry-after".to_owned(), "1".to_owned())],
                chunks: vec![Chunk::Data(
                    json!({"error": {"type": "rate_limit"}}).to_string().into(),
                )],
                expect_cancel: false,
            },
        }],
    })
    .await
    .unwrap();
    let controller = controller(1);
    let endpoint = fixture.endpoint();

    let slot = admitted_slot(&controller, 0);
    let mut client = fixture.connect(&endpoint).await.unwrap();
    let response = client
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await
        .unwrap();
    let status = response.status();
    collect(response).await.unwrap();
    let rejection = classify_status(status.as_u16());
    assert_eq!(rejection, ProviderRejection::RateLimited);
    assert!(
        !rejection.retry_eligible(),
        "429 must never be retry-eligible; Retry-After is ignored"
    );
    controller.release(slot, ReleaseKind::Completed);
    drop(client);

    // Exactly one request crossed the wire: the scenario had one step
    // and the report shows no unexpected traffic and no missing steps.
    let report = fixture.finish().await;
    assert!(
        report.failures.is_empty(),
        "strict simulator failures: {report:?}"
    );
    assert_eq!(report.requests, 1);
}

#[tokio::test]
async fn redirect_response_is_never_followed() {
    // The scenario scripts one 302. Following it would mean a second
    // request the simulator never scripted, which fails the run. The
    // client here treats the 302 as a terminal typed outcome.
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![Step {
            method: "POST".to_owned(),
            path: "/v1/chat/completions".to_owned(),
            headers: vec![],
            body: Some(json!({"model": "coder", "messages": [{"role": "user", "content": "1"}]})),
            reply: Reply {
                status: StatusCode::FOUND,
                headers: vec![("location".to_owned(), "/v1/chat/completions".to_owned())],
                chunks: vec![Chunk::Data("redirect".into())],
                expect_cancel: false,
            },
        }],
    })
    .await
    .unwrap();
    let controller = controller(1);
    let endpoint = fixture.endpoint();

    let slot = admitted_slot(&controller, 0);
    let mut client = fixture.connect(&endpoint).await.unwrap();
    let response = client
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await
        .unwrap();
    let status = response.status();
    collect(response).await.unwrap();
    assert!(
        status.is_redirection(),
        "redirect is surfaced as a terminal outcome, not followed"
    );
    assert_eq!(
        classify_status(status.as_u16()),
        ProviderRejection::Fatal,
        "redirect classification is fatal: no retry, no follow"
    );
    controller.release(slot, ReleaseKind::Completed);
    drop(client);

    let report = fixture.finish().await;
    assert!(
        report.failures.is_empty(),
        "strict simulator failures: {report:?}"
    );
    assert_eq!(
        report.requests, 1,
        "the redirect target was never contacted"
    );
}

#[tokio::test]
async fn a_retry_attempt_after_429_is_visible_as_unexpected_traffic() {
    // Negative control: if a caller violated the no-retry rule, the
    // strict simulator records it. One scripted 429, then an
    // unscripted second request must fail the report.
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![Step {
            method: "POST".to_owned(),
            path: "/v1/chat/completions".to_owned(),
            headers: vec![],
            body: Some(json!({"model": "coder", "messages": [{"role": "user", "content": "1"}]})),
            reply: Reply::bytes(
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error": {"type": "rate_limit"}}).to_string(),
            ),
        }],
    })
    .await
    .unwrap();
    let endpoint = fixture.endpoint();
    let mut client = fixture.connect(&endpoint).await.unwrap();
    let response = client
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await
        .unwrap();
    collect(response).await.unwrap();
    drop(client);
    // Violation on purpose: a second identical request.
    let mut retry_client = fixture.connect(&endpoint).await.unwrap();
    let response = retry_client
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await
        .unwrap();
    collect(response).await.unwrap();
    drop(retry_client);

    let report = fixture.finish().await;
    assert!(
        report
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::UnexpectedRequest { .. })),
        "the simulator must catch retry-like traffic: {report:?}"
    );
}

#[tokio::test]
async fn a_timed_out_queue_wait_sends_nothing() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(1)],
    })
    .await
    .unwrap();
    let controller = controller(1);

    // Occupy the only slot, then queue a request whose wait expires.
    let _blocker = admitted_slot(&controller, 0);
    let ticket = match controller.try_admit(
        &fluzo_runtime::admission::AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId("shared".to_owned()),
            model: ModelId("coder".to_owned()),
            input_estimate_tokens: 100,
            max_response_output_tokens: 50,
            remaining_task_seconds: 1,
            budgets: None,
            consumed: None,
        },
        0,
    ) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued, got {other:?}"),
    };
    assert_eq!(controller.expire(2_000), vec![ticket]);
    assert_eq!(
        controller.admit_next(&PoolId("shared".to_owned()), 2_000),
        None,
        "the expired waiter never dispatches"
    );
    // The fixture's single scripted step was never requested.
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0);
    assert!(
        report
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::MissingSteps { remaining: 1 })),
        "unsent work is visible as missing steps: {report:?}"
    );
}

#[tokio::test]
async fn budget_exhaustion_sends_nothing() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(1)],
    })
    .await
    .unwrap();
    let controller = controller(2);

    let mut req = fluzo_runtime::admission::AdmissionRequest {
        class: RequestClass::AgentTurn,
        pool: PoolId("shared".to_owned()),
        model: ModelId("coder".to_owned()),
        input_estimate_tokens: 100,
        max_response_output_tokens: 50,
        remaining_task_seconds: 3600,
        budgets: Some(fluzo_core::admission::TaskBudgets {
            max_turns: 1,
            max_active_seconds: 100,
            max_tool_calls: 10,
            max_total_tokens: 10_000,
            max_cost_microunits: None,
        }),
        consumed: Some(fluzo_core::admission::BudgetConsumption {
            turns: 1,
            ..Default::default()
        }),
    };
    assert!(
        matches!(
            controller.try_admit(&req, 0),
            Admission::Denied(AdmissionDenied::BudgetExhausted(_))
        ),
        "an exhausted task budget blocks the request before any socket"
    );
    req.consumed = None;
    assert_eq!(
        controller.try_admit(&req, 0),
        Admission::Denied(AdmissionDenied::BudgetsIncomplete),
        "a half-supplied budget pair is a typed error, not a silent pass"
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0);
}

#[tokio::test]
async fn a_cancelled_queue_entry_never_reaches_the_server() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(1)],
    })
    .await
    .unwrap();
    let controller = controller(1);

    let _blocker = admitted_slot(&controller, 0);
    let ticket = match controller.try_admit(
        &fluzo_runtime::admission::AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId("shared".to_owned()),
            model: ModelId("coder".to_owned()),
            input_estimate_tokens: 100,
            max_response_output_tokens: 50,
            remaining_task_seconds: 3600,
            budgets: None,
            consumed: None,
        },
        0,
    ) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued, got {other:?}"),
    };
    assert!(controller.cancel_queued(ticket));
    let report = fixture.finish().await;
    assert_eq!(
        report.requests, 0,
        "a cancelled waiter never opens a socket"
    );
    assert!(
        report
            .failures
            .iter()
            .any(|failure| matches!(failure, Failure::MissingSteps { remaining: 1 })),
        "the unsent scripted step stays visible: {report:?}"
    );
}

#[tokio::test]
async fn a_mid_stream_disconnect_leaves_the_slot_uncertain() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![Step {
            method: "POST".to_owned(),
            path: "/v1/chat/completions".to_owned(),
            headers: vec![],
            body: Some(json!({"model": "coder", "messages": [{"role": "user", "content": "1"}]})),
            reply: Reply {
                status: StatusCode::OK,
                headers: vec![],
                chunks: vec![Chunk::Data("partial".into()), Chunk::Disconnect],
                expect_cancel: false,
            },
        }],
    })
    .await
    .unwrap();
    let controller = controller(1);
    let endpoint = fixture.endpoint();

    let slot = admitted_slot(&controller, 0);
    let mut client = fixture.connect(&endpoint).await.unwrap();
    let response = client
        .request("POST", "/v1/chat/completions", chat_body(1).into())
        .await;
    // The scripted disconnect surfaces as a broken exchange: either
    // the response stream errors while collecting, or the connection
    // failed before a complete response. Both mean an unknown remote
    // outcome; neither is a clean success.
    let outcome = match response {
        Ok(response) => {
            assert_eq!(response.status(), StatusCode::OK);
            collect(response).await.map(|_| ())
        }
        Err(error) => Err(error),
    };
    assert!(outcome.is_err(), "stream broke before completion");
    drop(client);
    controller.release(slot, ReleaseKind::Uncertain);
    assert!(
        matches!(
            controller.try_admit(
                &fluzo_runtime::admission::AdmissionRequest {
                    class: RequestClass::AgentTurn,
                    pool: PoolId("shared".to_owned()),
                    model: ModelId("coder".to_owned()),
                    input_estimate_tokens: 100,
                    max_response_output_tokens: 50,
                    remaining_task_seconds: 3600,
                    budgets: None,
                    consumed: None,
                },
                1_000,
            ),
            Admission::Queued { .. }
        ),
        "the uncertain slot must not be treated as free capacity"
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "no extra traffic from the disconnect");
}
