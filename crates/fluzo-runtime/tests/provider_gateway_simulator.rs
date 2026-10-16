//! AGT-01 provider-gateway behavior against the strict loopback HTTP
//! simulator. Every acceptance-critical path (validated tool calls
//! before dispatch, usage-only final chunks, redirect blocking, 429
//! without retry, bounded transient retry, cancellation with
//! uncertain capacity, timeouts, consent, and telemetry) runs against
//! scripted fixtures that reject unexpected traffic.

#![allow(clippy::too_many_arguments)]

#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluzo_core::admission::{ProviderRejection, RequestClass};
use fluzo_core::model_discovery::AddressClass;
use fluzo_core::telemetry::ResourceIdentity;
use fluzo_runtime::admission::{
    Admission, AdmissionController, AdmissionDenied, ModelId, PoolId, PoolLimits,
};
use fluzo_runtime::providers::chat::{ChatError, build_chat_request};
use fluzo_runtime::providers::gateway::{
    AcceptedEndpoints, ChatRequest, ConsentError, ConsentGate, CredentialSource, GatewayConfig,
    GatewayError, ProviderGateway, TimeoutKind,
};
use fluzo_runtime::telemetry::{DiagnosticHub, DiagnosticSink, HubLimits, RedactionPolicy};
use hyper::StatusCode;
use serde_json::{Value, json};
use support::{Chunk, Fixture, Reply, Scenario, Step};
use tokio::sync::{oneshot, watch};

const MODEL: &str = "coder";
const SECRET: &str = "sk-test-secret-42";

#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl DiagnosticSink for Capture {
    fn name(&self) -> String {
        "capture".to_owned()
    }
    fn write_batch(&mut self, lines: &[String]) -> Result<(), String> {
        self.lines.lock().unwrap().extend(lines.iter().cloned());
        Ok(())
    }
}

struct ScriptedSecrets(Option<String>);

impl CredentialSource for ScriptedSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        self.0.clone()
    }
}

struct DenyAll;

impl ConsentGate for DenyAll {
    fn allow(&self, _endpoint: &str, class: AddressClass) -> Result<(), ConsentError> {
        Err(ConsentError::Denied { class })
    }
}

fn new_hub() -> (Arc<DiagnosticHub>, Capture) {
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    let capture = Capture::default();
    hub.add_sink(Box::new(capture.clone()));
    (Arc::new(hub), capture)
}

fn new_controller(queue_capacity: u32) -> AdmissionController {
    let controller = AdmissionController::new();
    controller
        .register_pool(
            PoolId("shared".to_owned()),
            PoolLimits::new(1, 1, queue_capacity).unwrap(),
            30_000,
            BTreeMap::from([(ModelId(MODEL.to_owned()), 1)]),
        )
        .unwrap();
    controller
}

fn gateway(
    endpoint: String,
    controller: Arc<AdmissionController>,
    hub: Arc<DiagnosticHub>,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    retries: u32,
) -> ProviderGateway {
    gateway_with_timeouts(
        endpoint,
        controller,
        hub,
        consent,
        credentials,
        retries,
        Duration::from_secs(2),
        Duration::from_secs(5),
        Duration::from_secs(5),
        Duration::from_secs(10),
    )
}

fn gateway_with_timeouts(
    endpoint: String,
    controller: Arc<AdmissionController>,
    hub: Arc<DiagnosticHub>,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    retries: u32,
    connect: Duration,
    first_output: Duration,
    idle: Duration,
    request: Duration,
) -> ProviderGateway {
    ProviderGateway::new(
        GatewayConfig {
            endpoint,
            api_key_env: Some("FLUZO_TEST_KEY".to_owned()),
            pool: PoolId("shared".to_owned()),
            capacity: ModelId(MODEL.to_owned()),
            retries,
            connect_timeout: connect,
            first_output_timeout: first_output,
            idle_stream_timeout: idle,
            request_timeout: request,
        },
        controller,
        consent,
        credentials,
        hub,
        ResourceIdentity {
            service_name: "fluzo-test".to_owned(),
            service_version: "0".to_owned(),
        },
    )
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        purpose: RequestClass::AgentTurn,
        model: MODEL.to_owned(),
        messages: vec![json!({"role": "user", "content": "hi"})],
        tools: None,
        max_output_tokens: 1024,
        input_estimate_tokens: 100,
        remaining_task_seconds: 3600,
        budgets: None,
        consumed: None,
    }
}

fn chat_step(reply: Reply) -> Step {
    Step {
        method: "POST".to_owned(),
        path: "/v1/chat/completions".to_owned(),
        headers: Vec::new(),
        body: Some(build_chat_request(
            MODEL,
            vec![json!({"role": "user", "content": "hi"})],
            None,
            1024,
        )),
        reply,
    }
}

fn sse(frames: &[Value]) -> String {
    let mut out = String::new();
    for frame in frames {
        out.push_str("data: ");
        out.push_str(&frame.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

fn stream_reply(frames: Vec<Value>) -> Reply {
    Reply {
        status: StatusCode::OK,
        headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
        chunks: vec![Chunk::Data(sse(&frames).into())],
        expect_cancel: false,
    }
}

fn empty_reply(status: StatusCode, headers: Vec<(String, String)>) -> Reply {
    Reply {
        status,
        headers,
        chunks: Vec::new(),
        expect_cancel: false,
    }
}

async fn run(
    gateway: &ProviderGateway,
    request: &ChatRequest,
) -> Result<fluzo_runtime::providers::gateway::GatewayResponse, GatewayError> {
    let (_tx, mut rx) = watch::channel(false);
    gateway.complete(request, &mut rx).await
}

fn success_frames() -> Vec<Value> {
    vec![
        json!({"id": "c1", "model": MODEL, "choices": [{"index": 0, "delta": {"content": "Hello "}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "world"}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}),
        json!({"usage": {"prompt_tokens": 120, "completion_tokens": 34}}),
    ]
}

#[tokio::test]
async fn streams_text_with_usage_only_final_chunk() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(stream_reply(success_frames()))],
    })
    .await
    .unwrap();
    let controller = Arc::new(new_controller(8));
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::clone(&controller),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
    );

    let response = run(&gw, &chat_request()).await.unwrap();
    assert_eq!(response.assembled.text, "Hello world");
    assert_eq!(response.assembled.finish_reasons, vec!["stop".to_owned()]);
    assert_eq!(response.usage.input_tokens, Some(120));
    assert_eq!(response.usage.output_tokens, Some(34));
    assert_eq!(response.attempts, 1);
    assert!(response.time_to_first_chunk.is_some());

    let view = controller.view(&PoolId("shared".to_owned()), 0).unwrap();
    assert_eq!(view.in_flight, 0);
    assert_eq!(view.uncertain, 0);

    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn fragmented_tool_calls_are_validated_before_dispatch() {
    let frames = vec![
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call-1", "function": {"name": "edit", "arguments": "{\"path\""}}
        ]}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": ": \"a.rs\"}"}}
        ]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        json!({"usage": {"prompt_tokens": 50, "completion_tokens": 20}}),
    ];
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(stream_reply(frames))],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
    );

    let response = run(&gw, &chat_request()).await.unwrap();
    assert_eq!(response.assembled.tool_calls.len(), 1);
    let call = &response.assembled.tool_calls[0];
    assert_eq!(call.id, "call-1");
    assert_eq!(call.name, "edit");
    let parsed: Value = serde_json::from_str(&call.arguments).expect("valid JSON arguments");
    assert_eq!(parsed["path"], "a.rs");
    let report = fixture.finish().await;
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn incomplete_tool_call_arguments_never_reach_dispatch() {
    let frames = vec![
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call-1", "function": {"name": "edit", "arguments": "{\"path\":"}}
        ]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
    ];
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(stream_reply(frames))],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert!(
        matches!(
            error,
            GatewayError::Stream(ChatError::InvalidToolCall { index: 0, .. })
        ),
        "got {error:?}"
    );
    let report = fixture.finish().await;
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn rate_limit_429_gets_no_retry_no_cooldown_no_fallback() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(empty_reply(
            StatusCode::TOO_MANY_REQUESTS,
            vec![("retry-after".to_owned(), "1".to_owned())],
        ))],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    // A retry budget exists and must NOT be spent on quota rejection.
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert_eq!(
        error,
        GatewayError::Rejected {
            status: 429,
            rejection: ProviderRejection::RateLimited,
        }
    );
    let report = fixture.finish().await;
    assert_eq!(
        report.requests, 1,
        "429 must never produce a second request"
    );
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn redirect_is_blocked_and_the_target_never_receives_traffic() {
    let mut target = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let location = target.endpoint();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(empty_reply(
            StatusCode::FOUND,
            vec![("location".to_owned(), location)],
        ))],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    assert_eq!(
        run(&gw, &chat_request()).await.unwrap_err(),
        GatewayError::RedirectBlocked
    );
    let target_report = target.finish().await;
    assert_eq!(
        target_report.requests, 0,
        "the redirect target must receive zero requests"
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn mid_stream_disconnect_after_deltas_is_partial_not_retried() {
    // The gate guarantees the delta reaches the client before the
    // scripted error: without it the fixture may abort the connection
    // before the response head flushes (see admission_simulator).
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(Reply {
            status: StatusCode::OK,
            headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
            chunks: vec![
                // No [DONE] sentinel: the stream is cut mid-flight.
                Chunk::Data(
                    format!(
                        "data: {}\n\n",
                        json!({"choices": [{"index": 0, "delta": {"content": "Partial"}}]})
                    )
                    .into(),
                ),
                Chunk::Gate(gate_rx),
                Chunk::Disconnect,
            ],
            expect_cancel: false,
        })],
    })
    .await
    .unwrap();
    let controller = Arc::new(new_controller(8));
    let (hub, _capture) = new_hub();
    // Generous retry budget must not trigger: deltas were surfaced.
    let gw = gateway(
        fixture.endpoint(),
        Arc::clone(&controller),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    let request = chat_request();
    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let (result, ()) = tokio::join!(gw.complete(&request, &mut cancel_rx), async {
        fixture.wait_for_chunks(1).await;
        gate_tx.send(()).unwrap();
    });
    let error = result.unwrap_err();
    match error {
        GatewayError::PartialStream(info) => {
            assert_eq!(info.text, "Partial");
            assert!(info.finish_reasons.is_empty());
            assert!(info.usage.is_none() || info.usage.unwrap().input_tokens.is_none());
        }
        other => panic!("expected PartialStream, got {other:?}"),
    }
    let view = controller.view(&PoolId("shared".to_owned()), 0).unwrap();
    assert_eq!(
        view.uncertain, 1,
        "a broken stream keeps the slot uncertain"
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "partial streams are never retried");
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn transient_5xx_retries_within_budget_and_succeeds() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![
            chat_step(empty_reply(StatusCode::SERVICE_UNAVAILABLE, Vec::new())),
            chat_step(stream_reply(success_frames())),
        ],
    })
    .await
    .unwrap();
    let controller = Arc::new(new_controller(8));
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::clone(&controller),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        1,
    );

    let response = run(&gw, &chat_request()).await.unwrap();
    assert_eq!(response.attempts, 2);
    assert_eq!(response.assembled.text, "Hello world");
    let view = controller.view(&PoolId("shared".to_owned()), 0).unwrap();
    assert_eq!(view.in_flight, 0);
    assert_eq!(view.uncertain, 0);
    let report = fixture.finish().await;
    assert_eq!(report.requests, 2);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn transient_budget_exhausted_surfaces_the_typed_error() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(empty_reply(StatusCode::BAD_GATEWAY, Vec::new()))],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert_eq!(
        error,
        GatewayError::Rejected {
            status: 502,
            rejection: ProviderRejection::Transient,
        }
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn cancellation_mid_stream_leaves_capacity_uncertain_and_sends_nothing_more() {
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(Reply {
            status: StatusCode::OK,
            headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
            chunks: vec![
                Chunk::Data(
                    sse(&[json!({"choices": [{"index": 0, "delta": {"content": "first"}}]})])
                        .into(),
                ),
                Chunk::Gate(gate_rx),
            ],
            expect_cancel: true,
        })],
    })
    .await
    .unwrap();
    let controller = Arc::new(new_controller(8));
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::clone(&controller),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let request = chat_request();
    let (result, ()) = tokio::join!(gw.complete(&request, &mut cancel_rx), async {
        fixture.wait_for_chunks(1).await;
        cancel_tx.send(true).unwrap();
    });
    assert_eq!(result.unwrap_err(), GatewayError::Cancelled);
    let view = controller.view(&PoolId("shared".to_owned()), 0).unwrap();
    assert_eq!(
        view.uncertain, 1,
        "cancelling after dispatch keeps the slot uncertain"
    );
    drop(gate_tx);
    fixture.wait_for_idle().await;
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "cancellation must not trigger a retry");
    assert_eq!(report.cancelled, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn first_output_timeout_fails_typed_without_retrying_forever() {
    let (_gate_tx, gate_rx) = oneshot::channel::<()>();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![chat_step(Reply {
            status: StatusCode::OK,
            headers: vec![("content-type".to_owned(), "text/event-stream".to_owned())],
            chunks: vec![Chunk::Gate(gate_rx)],
            expect_cancel: true,
        })],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway_with_timeouts(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
        Duration::from_secs(2),
        Duration::from_millis(200),
        Duration::from_secs(5),
        Duration::from_secs(10),
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert_eq!(error, GatewayError::Timeout(TimeoutKind::FirstOutput));
    fixture.wait_for_idle().await;
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn admission_exhaustion_never_opens_a_socket() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let controller = Arc::new(new_controller(0));
    // Occupy the only slot before the gateway call.
    let held = match controller.try_admit(
        &fluzo_runtime::admission::AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId("shared".to_owned()),
            model: ModelId(MODEL.to_owned()),
            input_estimate_tokens: 1,
            max_response_output_tokens: 1,
            remaining_task_seconds: 60,
            budgets: None,
            consumed: None,
        },
        0,
    ) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::clone(&controller),
        hub,
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert_eq!(
        error,
        GatewayError::Admission(AdmissionDenied::CapacityExhausted)
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0, "no capacity means no packets");
    assert_eq!(report.failures, vec![]);
    let _ = held;
}

#[tokio::test]
async fn consent_denial_stops_before_any_socket() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        hub,
        Arc::new(DenyAll),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        2,
    );

    let error = run(&gw, &chat_request()).await.unwrap_err();
    assert_eq!(
        error,
        GatewayError::Consent(ConsentError::Denied {
            class: AddressClass::Loopback,
        })
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn telemetry_records_pinned_genai_signals_without_content_or_secrets() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![Step {
            method: "POST".to_owned(),
            path: "/v1/chat/completions".to_owned(),
            headers: vec![("authorization".to_owned(), format!("Bearer {SECRET}"))],
            body: Some(build_chat_request(
                MODEL,
                vec![json!({"role": "user", "content": "hi"})],
                None,
                1024,
            )),
            reply: stream_reply(success_frames()),
        }],
    })
    .await
    .unwrap();
    let (mut hub, capture) = new_hub();
    let gw = gateway(
        fixture.endpoint(),
        Arc::new(new_controller(8)),
        Arc::clone(&hub),
        Arc::new(AcceptedEndpoints::new()),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        0,
    );

    run(&gw, &chat_request()).await.unwrap();
    drop(gw);
    let hub_mut = Arc::get_mut(&mut hub).expect("gateway released the hub");
    let _ = hub_mut.shutdown(Duration::from_secs(2));

    let lines = capture.lines.lock().unwrap().join("\n");
    assert!(lines.contains("chat coder"), "pinned span name missing");
    assert!(
        lines.contains("gen_ai.client.operation.duration"),
        "duration metric missing"
    );
    assert!(
        lines.contains("gen_ai.client.operation.time_to_first_chunk"),
        "TTFC metric missing"
    );
    assert!(
        lines.contains("gen_ai.client.inference.usage.input_tokens"),
        "usage metric missing"
    );
    assert!(
        lines.contains("gen_ai.response.finish_reasons"),
        "finish reasons missing"
    );
    assert!(lines.contains("agent_turn"), "purpose attribute missing");
    assert!(
        !lines.contains(SECRET),
        "the credential value must never reach telemetry"
    );
    assert!(
        !lines.contains("gen_ai.input.messages") && !lines.contains("gen_ai.output.messages"),
        "content attributes are opt-in and must stay absent"
    );
    let report = fixture.finish().await;
    assert_eq!(report.failures, vec![]);
}
