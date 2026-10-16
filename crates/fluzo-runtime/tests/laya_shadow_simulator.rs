//! AGT-03 Laya shadow adapter against the strict loopback HTTP
//! simulator. Every acceptance-critical path (typed question wire
//! format, answer validation, consent before any socket, skip-not-
//! queue admission, redirect blocking with zero traffic to the
//! target, 429 without retry, timeout and cancellation leaving the
//! slot uncertain, response ceilings and telemetry) runs against
//! scripted fixtures that reject unexpected traffic.

#![allow(clippy::too_many_arguments)]

#[allow(dead_code)]
mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fluzo_core::admission::RequestClass;
use fluzo_core::model_discovery::AddressClass;
use fluzo_core::telemetry::ResourceIdentity;
use fluzo_runtime::admission::{
    Admission, AdmissionController, AdmissionRequest, ModelId, PoolId, PoolLimits,
};
use fluzo_runtime::providers::gateway::{ConsentError, ConsentGate, CredentialSource};
use fluzo_runtime::shadow::coordinator::{CoordinatorLimits, ShadowCoordinator, Submission};
use fluzo_runtime::shadow::observer::{LayaConfig, LayaShadowObserver};
use fluzo_runtime::shadow::wire::{self, ShadowQuestion};
use fluzo_runtime::shadow::{DecisionPoint, ObservationStatus, ShadowRequest};
use fluzo_runtime::telemetry::{DiagnosticHub, DiagnosticSink, HubLimits, RedactionPolicy};
use hyper::StatusCode;
use serde_json::{Value, json};
use support::{Chunk, Fixture, Reply, Scenario, Step};
use tokio::sync::{oneshot, watch};

const SECRET: &str = "sk-laya-test-77";
const POOL: &str = "shared";
const MODEL: &str = "laya-mini";

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

/// One total slot with the foreground share reserved leaves the
/// shadow allowance at zero; `allowance = total - reserved`.
fn controller(total: u32) -> Arc<AdmissionController> {
    let controller = AdmissionController::new();
    controller
        .register_pool(
            PoolId(POOL.to_owned()),
            PoolLimits::new(total, 1, 4).unwrap(),
            30_000,
            BTreeMap::from([(ModelId(MODEL.to_owned()), total)]),
        )
        .unwrap();
    Arc::new(controller)
}

fn choice_request() -> ShadowRequest {
    ShadowRequest {
        observation_id: "obs-sim-1".to_owned(),
        decision_point: DecisionPoint {
            task: "task-1".to_owned(),
            stage: "pre-tool".to_owned(),
            state_version: 4,
            state_fingerprint: "fp-4".to_owned(),
        },
        question: ShadowQuestion::Choice {
            prompt: "next action?".to_owned(),
            options: vec!["edit".to_owned(), "ask".to_owned(), "stop".to_owned()],
        },
        model: MODEL.to_owned(),
        input_estimate_tokens: 120,
        remaining_task_seconds: 60,
        state_size_bytes: 2048,
        language: Some("en".to_owned()),
        truncation: None,
    }
}

fn expected_body(request: &ShadowRequest) -> Value {
    wire::build_request_json(
        &request.observation_id,
        &request.question,
        &request.decision_point.to_json(),
    )
}

fn systemone_step(body: Value, reply: Reply) -> Step {
    Step {
        method: "POST".to_owned(),
        path: "/v1/systemone".to_owned(),
        headers: vec![
            ("authorization".to_owned(), format!("Bearer {SECRET}")),
            ("content-type".to_owned(), "application/json".to_owned()),
        ],
        body: Some(body),
        reply,
    }
}

fn json_reply(body: Value) -> Reply {
    Reply {
        status: StatusCode::OK,
        headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        chunks: vec![Chunk::Data(body.to_string().into())],
        expect_cancel: false,
    }
}

fn observer(
    endpoint: String,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    admission: Arc<AdmissionController>,
    timeout: Duration,
) -> LayaShadowObserver {
    let (hub, _capture) = new_hub();
    LayaShadowObserver::new(
        LayaConfig {
            endpoint,
            api_key_env: Some("FLUZO_TEST_KEY".to_owned()),
            model: MODEL.to_owned(),
            pool: PoolId(POOL.to_owned()),
            capacity: ModelId(MODEL.to_owned()),
            timeout,
            response_ceiling_bytes: 64 * 1024,
            max_response_output_tokens: 64,
        },
        admission,
        consent,
        credentials,
        hub,
        ResourceIdentity {
            service_name: "fluzo-test".to_owned(),
            service_version: "0".to_owned(),
        },
    )
}

async fn observe(
    observer: &LayaShadowObserver,
    request: ShadowRequest,
) -> fluzo_runtime::shadow::ShadowObservationRecord {
    let (_tx, rx) = watch::channel(false);
    observer.run(request, rx).await
}

fn choice_answer(answer: &str) -> Value {
    json!({
        "schema_version": 1,
        "type": "choice",
        "answer": answer,
        "probabilities": {"edit": 0.7, "ask": 0.2, "stop": 0.1},
        "confidence": 0.8,
        "action": {"recommended": answer, "act_probability": 0.4}
    })
}

#[tokio::test]
async fn happy_choice_round_trip_records_completed_observation() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            json_reply(choice_answer("edit")),
        )],
    })
    .await
    .unwrap();
    let admission = controller(2);
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        Arc::clone(&admission),
        Duration::from_secs(2),
    );

    let record = observe(&observer, request).await;
    assert_eq!(record.status, ObservationStatus::Completed);
    assert_eq!(record.recommended_action.as_deref(), Some("edit"));
    assert!(record.answer.is_some());
    let view = admission.view(&PoolId(POOL.to_owned()), 0).unwrap();
    assert_eq!(view.in_flight, 0);
    assert_eq!(view.uncertain, 0);
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.consumed, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn noul_and_score_answers_round_trip() {
    let noul_request = ShadowRequest {
        observation_id: "obs-noul".to_owned(),
        question: ShadowQuestion::Noul {
            prompt: "allowed?".to_owned(),
        },
        ..choice_request()
    };
    let score_request = ShadowRequest {
        observation_id: "obs-score".to_owned(),
        question: ShadowQuestion::Score {
            prompt: "rank".to_owned(),
            candidates: vec!["a".to_owned(), "b".to_owned()],
        },
        ..choice_request()
    };
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![
            systemone_step(
                expected_body(&noul_request),
                json_reply(json!({
                    "schema_version": 1, "type": "noul", "answer": true,
                    "confidence": 0.9,
                    "action": {"recommended": "yes", "act_probability": 0.3}
                })),
            ),
            systemone_step(
                expected_body(&score_request),
                json_reply(json!({
                    "schema_version": 1, "type": "score",
                    "probabilities": {"a": 0.6, "b": 0.4},
                    "confidence": 0.7,
                    "action": {"recommended": "a", "act_probability": 0.5}
                })),
            ),
        ],
    })
    .await
    .unwrap();
    let admission = controller(2);
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        Arc::clone(&admission),
        Duration::from_secs(2),
    );

    let noul = observe(&observer, noul_request).await;
    assert_eq!(noul.status, ObservationStatus::Completed);
    let score = observe(&observer, score_request).await;
    assert_eq!(score.status, ObservationStatus::Completed);
    let report = fixture.finish().await;
    assert_eq!(report.requests, 2);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn unknown_answer_type_is_rejected_without_retry() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            json_reply(json!({"schema_version": 1, "type": "vibes"})),
        )],
    })
    .await
    .unwrap();
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Rejected {
            reason: wire::AnswerRejection::UnknownAnswerType
        }
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "rejection must not trigger a retry");
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn out_of_set_choice_is_rejected() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            json_reply(choice_answer("delete_everything")),
        )],
    })
    .await
    .unwrap();
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Rejected {
            reason: wire::AnswerRejection::ChoiceOutOfSet
        }
    );
    fixture.finish().await;
}

#[tokio::test]
async fn redirect_is_blocked_with_zero_traffic_to_the_target() {
    let mut target = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            Reply {
                status: StatusCode::FOUND,
                headers: vec![("location".to_owned(), target.endpoint())],
                chunks: vec![Chunk::Data("redirecting".into())],
                expect_cancel: false,
            },
        )],
    })
    .await
    .unwrap();
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "redirect_blocked".to_owned()
        }
    );
    let target_report = target.finish().await;
    assert_eq!(
        target_report.requests, 0,
        "the redirect target must never be contacted"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn rate_limit_429_gets_no_retry() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            Reply::bytes(StatusCode::TOO_MANY_REQUESTS, "slow down"),
        )],
    })
    .await
    .unwrap();
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "rate_limited".to_owned()
        }
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "429 must not trigger any retry");
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn timeout_leaves_the_admission_slot_uncertain() {
    let (_gate_tx, gate_rx) = oneshot::channel::<()>();
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            Reply {
                status: StatusCode::OK,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                chunks: vec![Chunk::Gate(gate_rx)],
                expect_cancel: true,
            },
        )],
    })
    .await
    .unwrap();
    let admission = controller(2);
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        Arc::clone(&admission),
        Duration::from_millis(250),
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Timeout {
            stage: "request".to_owned()
        }
    );
    let view = admission.view(&PoolId(POOL.to_owned()), 0).unwrap();
    assert_eq!(
        view.uncertain, 1,
        "a timed-out dispatched request keeps its slot uncertain"
    );
    fixture.wait_for_idle().await;
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn cancellation_after_dispatch_leaves_the_slot_uncertain() {
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            Reply {
                status: StatusCode::OK,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                chunks: vec![Chunk::Gate(gate_rx)],
                expect_cancel: true,
            },
        )],
    })
    .await
    .unwrap();
    let admission = controller(2);
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        Arc::clone(&admission),
        Duration::from_secs(5),
    );

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let run = tokio::spawn(async move { observer.run(request, cancel_rx).await });
    fixture.wait_for_connection().await;
    // Let the request finish parsing server-side before cancelling so
    // the fixture counts exactly one request.
    tokio::time::sleep(Duration::from_millis(50)).await;
    cancel_tx.send(true).unwrap();
    let record = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("observer must react to cancellation")
        .unwrap();
    assert_eq!(
        record.status,
        ObservationStatus::Cancelled {
            stage: "after_dispatch".to_owned()
        }
    );
    let view = admission.view(&PoolId(POOL.to_owned()), 0).unwrap();
    assert_eq!(
        view.uncertain, 1,
        "cancelling after dispatch keeps the slot uncertain"
    );
    drop(gate_tx);
    fixture.wait_for_idle().await;
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1, "cancellation must not trigger a retry");
    assert_eq!(report.failures, vec![]);
}

#[tokio::test]
async fn consent_denial_opens_no_socket() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let observer = observer(
        fixture.endpoint(),
        Arc::new(DenyAll),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    );
    let record = observe(&observer, choice_request()).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "consent_denied".to_owned()
        }
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0, "denied consent must send nothing");
}

#[tokio::test]
async fn no_shadow_allowance_skips_without_traffic() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    // total == reserved: the shadow allowance is zero.
    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(1),
        Duration::from_secs(2),
    );
    let record = observe(&observer, choice_request()).await;
    assert_eq!(
        record.status,
        ObservationStatus::Skipped {
            reason: "no_shadow_allowance".to_owned()
        }
    );
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0, "a skipped observation sends nothing");
}

#[tokio::test]
async fn queued_foreground_makes_shadow_skip_immediately() {
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: Vec::new(),
    })
    .await
    .unwrap();
    let admission = controller(1);
    // Occupy the only slot with foreground work and queue another
    // foreground request; shadow must skip with foreground_queued.
    let held = match admission.try_admit(
        &AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId(POOL.to_owned()),
            model: ModelId(MODEL.to_owned()),
            input_estimate_tokens: 10,
            max_response_output_tokens: 10,
            remaining_task_seconds: 30,
            budgets: None,
            consumed: None,
        },
        0,
    ) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let queued = admission.try_admit(
        &AdmissionRequest {
            class: RequestClass::AgentTurn,
            pool: PoolId(POOL.to_owned()),
            model: ModelId(MODEL.to_owned()),
            input_estimate_tokens: 10,
            max_response_output_tokens: 10,
            remaining_task_seconds: 30,
            budgets: None,
            consumed: None,
        },
        0,
    );
    assert!(matches!(queued, Admission::Queued { .. }));

    let observer = observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        Arc::clone(&admission),
        Duration::from_secs(2),
    );
    let record = observe(&observer, choice_request()).await;
    assert_eq!(
        record.status,
        ObservationStatus::Skipped {
            reason: "foreground_queued".to_owned()
        }
    );
    admission.release(held, fluzo_runtime::admission::ReleaseKind::Completed);
    let report = fixture.finish().await;
    assert_eq!(report.requests, 0, "a skipped observation sends nothing");
}

#[tokio::test]
async fn oversized_response_is_rejected_at_the_ceiling() {
    let request = choice_request();
    // The fixture caps scenario bytes at 64 KiB, so the observer's
    // ceiling is lowered instead of the payload being raised.
    let huge = "x".repeat(32 * 1024);
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            Reply {
                status: StatusCode::OK,
                headers: vec![("content-type".to_owned(), "application/json".to_owned())],
                chunks: vec![Chunk::Data(huge.into())],
                expect_cancel: false,
            },
        )],
    })
    .await
    .unwrap();
    let (hub, _capture) = new_hub();
    let observer = LayaShadowObserver::new(
        LayaConfig {
            endpoint: fixture.endpoint(),
            api_key_env: Some("FLUZO_TEST_KEY".to_owned()),
            model: MODEL.to_owned(),
            pool: PoolId(POOL.to_owned()),
            capacity: ModelId(MODEL.to_owned()),
            timeout: Duration::from_secs(2),
            response_ceiling_bytes: 8 * 1024,
            max_response_output_tokens: 64,
        },
        controller(2),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        hub,
        ResourceIdentity {
            service_name: "fluzo-test".to_owned(),
            service_version: "0".to_owned(),
        },
    );
    let record = observe(&observer, request).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "response_too_large".to_owned()
        }
    );
    fixture.finish().await;
}

#[tokio::test]
async fn telemetry_records_shadow_span_without_leaking_the_credential() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            json_reply(choice_answer("ask")),
        )],
    })
    .await
    .unwrap();
    let (hub, capture) = new_hub();
    let observer = std::sync::Arc::new(LayaShadowObserver::new(
        LayaConfig {
            endpoint: fixture.endpoint(),
            api_key_env: Some("FLUZO_TEST_KEY".to_owned()),
            model: MODEL.to_owned(),
            pool: PoolId(POOL.to_owned()),
            capacity: ModelId(MODEL.to_owned()),
            timeout: Duration::from_secs(2),
            response_ceiling_bytes: 64 * 1024,
            max_response_output_tokens: 64,
        },
        controller(2),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        hub,
        ResourceIdentity {
            service_name: "fluzo-test".to_owned(),
            service_version: "0".to_owned(),
        },
    ));
    let (_tx, rx) = watch::channel(false);
    let record = observer.run(request, rx).await;
    assert_eq!(record.status, ObservationStatus::Completed);
    let joined = capture.lines.lock().unwrap().join("\n");
    assert!(
        joined.contains("systemone"),
        "expected a systemone span in telemetry: {joined}"
    );
    assert!(joined.contains("shadow_observation"));
    assert!(
        !joined.contains(SECRET),
        "the credential value must never reach telemetry"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn coordinator_end_to_end_with_the_real_observer() {
    let request = choice_request();
    let mut fixture = Fixture::start(Scenario {
        version: 1,
        steps: vec![systemone_step(
            expected_body(&request),
            json_reply(choice_answer("stop")),
        )],
    })
    .await
    .unwrap();
    let observer = Arc::new(observer(
        fixture.endpoint(),
        Arc::new(fluzo_runtime::providers::gateway::LoopbackOnlyConsent),
        Arc::new(ScriptedSecrets(Some(SECRET.to_owned()))),
        controller(2),
        Duration::from_secs(2),
    ));
    let sink = Arc::new(fluzo_runtime::shadow::coordinator::CollectingSink::default());
    let dyn_sink: Arc<dyn fluzo_runtime::shadow::ObservationSink> = sink.clone();
    let coordinator = Arc::new(ShadowCoordinator::new(
        CoordinatorLimits::default(),
        observer,
        dyn_sink,
    ));
    coordinator.start();
    coordinator.set_enabled(true);
    assert_eq!(coordinator.submit(request), Submission::Accepted);
    let started = std::time::Instant::now();
    let records = loop {
        let records = sink.records();
        if !records.is_empty() {
            break records;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "coordinator never produced a record"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert_eq!(records[0].status, ObservationStatus::Completed);
    assert_eq!(records[0].recommended_action.as_deref(), Some("stop"));
    let report = fixture.finish().await;
    assert_eq!(report.requests, 1);
    assert_eq!(report.failures, vec![]);
}
