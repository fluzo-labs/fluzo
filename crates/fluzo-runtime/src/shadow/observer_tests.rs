use std::collections::BTreeMap;
use std::sync::Arc;

use fluzo_core::model_discovery::AddressClass;
use fluzo_core::telemetry::ResourceIdentity;

use crate::admission::{AdmissionController, ModelId, PoolId, PoolLimits, ReleaseKind};
use crate::providers::gateway::{ConsentError, LoopbackOnlyConsent};
use crate::shadow::wire::ShadowQuestion;
use crate::shadow::{DecisionPoint, ObservationStatus, ShadowRequest};
use crate::telemetry::{DiagnosticHub, HubLimits, RedactionPolicy};

use super::*;

struct DenyAll;

impl ConsentGate for DenyAll {
    fn allow(&self, _endpoint: &str, class: AddressClass) -> Result<(), ConsentError> {
        Err(ConsentError::Denied { class })
    }
}

struct NoSecrets;

impl CredentialSource for NoSecrets {
    fn resolve(&self, _name: &str) -> Option<String> {
        None
    }
}

struct FixedSecret<'a>(&'a str);

impl CredentialSource for FixedSecret<'_> {
    fn resolve(&self, _name: &str) -> Option<String> {
        Some(self.0.to_owned())
    }
}

fn hub() -> Arc<DiagnosticHub> {
    Arc::new(DiagnosticHub::new(
        HubLimits::default(),
        RedactionPolicy::default(),
    ))
}

fn resource() -> ResourceIdentity {
    ResourceIdentity {
        service_name: "fluzo-test".to_owned(),
        service_version: "0".to_owned(),
    }
}

/// One total slot with the foreground share reserved leaves the
/// shadow allowance at zero; widening the pool grants one.
fn controller(shadow_allowance: bool) -> Arc<AdmissionController> {
    let controller = AdmissionController::new();
    let total = if shadow_allowance { 2 } else { 1 };
    controller
        .register_pool(
            PoolId("shared".to_owned()),
            PoolLimits::new(total, 1, 4).unwrap(),
            30_000,
            BTreeMap::from([(ModelId("laya-mini".to_owned()), total)]),
        )
        .unwrap();
    Arc::new(controller)
}

fn config(endpoint: &str) -> LayaConfig {
    LayaConfig {
        endpoint: endpoint.to_owned(),
        api_key_env: Some("FLUZO_TEST_KEY".to_owned()),
        model: "laya-mini".to_owned(),
        pool: PoolId("shared".to_owned()),
        capacity: ModelId("laya-mini".to_owned()),
        timeout: Duration::from_millis(300),
        response_ceiling_bytes: 64 * 1024,
        max_response_output_tokens: 64,
    }
}

fn observer(
    endpoint: &str,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    admission: Arc<AdmissionController>,
) -> Arc<LayaShadowObserver> {
    Arc::new(LayaShadowObserver::new(
        config(endpoint),
        admission,
        consent,
        credentials,
        hub(),
        resource(),
    ))
}

fn request() -> ShadowRequest {
    ShadowRequest {
        observation_id: "obs-unit".to_owned(),
        decision_point: DecisionPoint {
            task: "task".to_owned(),
            stage: "stage".to_owned(),
            state_version: 1,
            state_fingerprint: "fp".to_owned(),
        },
        question: ShadowQuestion::Noul {
            prompt: "proceed?".to_owned(),
        },
        model: "laya-mini".to_owned(),
        input_estimate_tokens: 10,
        remaining_task_seconds: 30,
        state_size_bytes: 100,
        language: None,
        truncation: None,
    }
}

#[tokio::test]
async fn invalid_endpoint_fails_before_any_dispatch() {
    let observer = observer(
        "not-an-endpoint",
        Arc::new(LoopbackOnlyConsent),
        Arc::new(FixedSecret("s3cret")),
        controller(true),
    );
    let record = observer.observe(request(), watch::channel(false).1).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "invalid_endpoint".to_owned()
        }
    );
}

#[tokio::test]
async fn consent_denial_produces_failed_record() {
    let observer = observer(
        "http://127.0.0.1:9/v1",
        Arc::new(DenyAll),
        Arc::new(FixedSecret("s3cret")),
        controller(true),
    );
    let record = observer.observe(request(), watch::channel(false).1).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "consent_denied".to_owned()
        }
    );
}

#[tokio::test]
async fn missing_credential_fails_without_dispatch() {
    let observer = observer(
        "http://127.0.0.1:9/v1",
        Arc::new(LoopbackOnlyConsent),
        Arc::new(NoSecrets),
        controller(true),
    );
    let record = observer.observe(request(), watch::channel(false).1).await;
    assert_eq!(
        record.status,
        ObservationStatus::Failed {
            code: "credential_unavailable".to_owned()
        }
    );
}

#[tokio::test]
async fn no_shadow_allowance_skips_without_dispatch() {
    let observer = observer(
        "http://127.0.0.1:9/v1",
        Arc::new(LoopbackOnlyConsent),
        Arc::new(FixedSecret("s3cret")),
        controller(false),
    );
    let record = observer.observe(request(), watch::channel(false).1).await;
    assert_eq!(
        record.status,
        ObservationStatus::Skipped {
            reason: "no_shadow_allowance".to_owned()
        }
    );
}

#[tokio::test]
async fn unknown_pool_is_a_configuration_failure_not_a_skip() {
    let empty = AdmissionController::new();
    let observer = Arc::new(LayaShadowObserver::new(
        LayaConfig {
            pool: PoolId("missing".to_owned()),
            ..config("http://127.0.0.1:9/v1")
        },
        Arc::new(empty),
        Arc::new(LoopbackOnlyConsent),
        Arc::new(FixedSecret("s3cret")),
        hub(),
        resource(),
    ));
    let record = observer.observe(request(), watch::channel(false).1).await;
    match &record.status {
        ObservationStatus::Failed { code } => {
            assert!(
                code.starts_with("admission_denied:"),
                "unexpected status: {}",
                record.status.code()
            );
        }
        other => {
            let shown = format!("{other:?}");
            panic!("expected admission failure, got {shown}")
        }
    }
}

#[test]
fn skip_codes_map_admission_outcomes() {
    use fluzo_core::admission::ShadowSkipReason;
    assert_eq!(
        skip_code(ShadowSkipReason::ForegroundQueued),
        "foreground_queued"
    );
    assert_eq!(
        skip_code(ShadowSkipReason::NoShadowAllowance),
        "no_shadow_allowance"
    );
    assert_eq!(skip_code(ShadowSkipReason::ModelAtLimit), "model_at_limit");
}

#[test]
fn release_kind_is_uncertain_only_after_dispatch() {
    assert_eq!(release_kind(false), ReleaseKind::Completed);
    assert_eq!(release_kind(true), ReleaseKind::Uncertain);
}

#[test]
fn non_loopback_endpoints_are_rejected_by_consent_gate_shape() {
    // The adapter asks the gate about the resolved class; a
    // loopback-only gate denies public addresses. The endpoint parse
    // itself still accepts the shape so the denial is attributed to
    // consent, not to a silent rewrite.
    let target = chat_target("http://203.0.113.7:8080/v1");
    assert!(target.is_ok());
    let gate = LoopbackOnlyConsent;
    assert!(
        gate.allow("http://203.0.113.7:8080/v1", AddressClass::Public)
            .is_err()
    );
    assert!(
        gate.allow("http://127.0.0.1:8080/v1/systemone", AddressClass::Loopback)
            .is_ok()
    );
}
