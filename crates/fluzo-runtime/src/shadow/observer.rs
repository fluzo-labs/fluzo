//! Laya `/v1/systemone` adapter.
//!
//! Composes the AGT-01 gateway-owned gates without being a Chat
//! Completions client: endpoint parsing, address classification and
//! consent run before any socket; admission uses the
//! `ShadowObservation` skip-not-queue rules; the credential value is
//! only read into a sensitive header; the whole observation has one
//! bounded deadline and never retries. After dispatch, a timeout or
//! transport break leaves the admission slot `Uncertain` — dropping
//! the future is not proof the model stopped (PRD 43.3).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fluzo_core::admission::{RequestClass, ShadowSkipReason};
use fluzo_core::model_discovery::AddressClass;
use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, ResourceIdentity, SECONDS_UNIT, SignalKind, SpanKind,
    SpanRecord, SpanStatus, attributes, gen_ai_span_name, instruments,
};
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::HeaderValue;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::admission::{
    Admission, AdmissionController, AdmissionDenied, AdmissionRequest, ModelId, PoolId, ReleaseKind,
};
use crate::model_discovery::classify;
use crate::providers::gateway::{
    self, ChatTarget, ConsentGate, CredentialSource, bearer_authorization, chat_target, resolve,
};
use crate::telemetry::DiagnosticHub;

use super::wire::{self, ShadowAnswer, ShadowQuestion};
use super::{DecisionObserver, ObservationStatus, ShadowObservationRecord, ShadowRequest};

/// Provider protocol identity for telemetry (matches the settings
/// `Provider` choice).
pub const LAYA_PROTOCOL: &str = "laya_systemone";

/// Static adapter configuration mapped from the typed settings.
pub struct LayaConfig {
    /// Base endpoint (`http://host:port` or `http://host:port/v1`).
    pub endpoint: String,
    pub api_key_env: Option<String>,
    pub model: String,
    pub pool: PoolId,
    pub capacity: ModelId,
    /// Whole-observation deadline (`decision.timeout_seconds`); no
    /// automatic retry follows it.
    pub timeout: Duration,
    /// Hard cap on the response body actually read.
    pub response_ceiling_bytes: usize,
    /// Reserved output for the admission estimate.
    pub max_response_output_tokens: u64,
}

/// `DecisionObserver` implementation speaking `/v1/systemone`.
pub struct LayaShadowObserver {
    config: LayaConfig,
    admission: Arc<AdmissionController>,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    hub: Arc<DiagnosticHub>,
    resource: ResourceIdentity,
}

impl LayaShadowObserver {
    pub fn new(
        config: LayaConfig,
        admission: Arc<AdmissionController>,
        consent: Arc<dyn ConsentGate>,
        credentials: Arc<dyn CredentialSource>,
        hub: Arc<DiagnosticHub>,
        resource: ResourceIdentity,
    ) -> Self {
        Self {
            config,
            admission,
            consent,
            credentials,
            hub,
            resource,
        }
    }

    /// One observation to its explicit terminal status. Never
    /// retries: every failure path below is terminal for this
    /// observation.
    pub async fn run(
        &self,
        request: ShadowRequest,
        cancel: watch::Receiver<bool>,
    ) -> ShadowObservationRecord {
        let started = Instant::now();
        let mut record = request.base_record();
        match self.attempt(&request, cancel).await {
            Ok(answer) => {
                record.recommended_action = Some(answer_action(&answer));
                record.answer = Some(answer);
                record.status = ObservationStatus::Completed;
            }
            Err(status) => record.status = status,
        }
        self.emit(&record, started);
        record
    }

    async fn attempt(
        &self,
        request: &ShadowRequest,
        mut cancel: watch::Receiver<bool>,
    ) -> Result<ShadowAnswer, ObservationStatus> {
        let target = chat_target(&self.config.endpoint).map_err(|_| failed("invalid_endpoint"))?;
        // Resolve once and connect to exactly the addresses the
        // consent decision was made about (same discipline as the
        // chat gateway).
        let addresses = resolve(&target).await;
        let class = classify(&addresses);
        if class == AddressClass::Unresolved {
            return Err(failed("transport"));
        }
        // Consent before any socket: a denial means zero packets left
        // the process for this observation.
        self.consent
            .allow(&self.config.endpoint, class)
            .map_err(|_| failed("consent_denied"))?;
        let authorization = bearer_authorization(
            self.config.api_key_env.as_deref(),
            self.credentials.as_ref(),
        )
        .map_err(|error| failed(&credential_code(error)))?;

        let body = Bytes::from(
            wire::build_request_json(
                &request.observation_id,
                &request.question,
                &request.decision_point.to_json(),
            )
            .to_string(),
        );

        let admission_request = AdmissionRequest {
            class: RequestClass::ShadowObservation,
            pool: self.config.pool.clone(),
            model: self.config.capacity.clone(),
            input_estimate_tokens: request.input_estimate_tokens,
            max_response_output_tokens: self.config.max_response_output_tokens,
            remaining_task_seconds: request.remaining_task_seconds,
            budgets: None,
            consumed: None,
        };
        let slot = match self
            .admission
            .try_admit(&admission_request, unix_ms() as u64)
        {
            Admission::Admitted(slot) => slot,
            Admission::Denied(AdmissionDenied::ShadowSkipped { reason }) => {
                return Err(ObservationStatus::Skipped {
                    reason: skip_code(reason).to_owned(),
                });
            }
            Admission::Denied(denied) => {
                return Err(failed(&format!("admission_denied:{}", denied.code())));
            }
            // The pure rules never queue shadow work; a queue outcome
            // here would be a contract break and stays visible.
            Admission::Queued { .. } => return Err(failed("admission_unexpected_queue")),
        };

        // Whether the request body reached the transport decides the
        // slot release kind after a break.
        let dispatched = Arc::new(AtomicBool::new(false));
        let exchange = self.exchange(
            &target,
            &addresses,
            body,
            authorization,
            &dispatched,
            &request.question,
        );
        let outcome = tokio::select! {
            biased;
            _ = cancel.wait_for(|cancelled| *cancelled) => {
                let kind = release_kind(dispatched.load(Ordering::SeqCst));
                self.admission.release(slot, kind);
                let stage = if kind == ReleaseKind::Uncertain { "after_dispatch" } else { "before_dispatch" };
                Err(ObservationStatus::Cancelled { stage: stage.to_owned() })
            }
            result = tokio::time::timeout(self.config.timeout, exchange) => match result {
                Err(_elapsed) => {
                    let kind = release_kind(dispatched.load(Ordering::SeqCst));
                    self.admission.release(slot, kind);
                    Err(ObservationStatus::Timeout { stage: "request".to_owned() })
                }
                Ok(Err(ExchangeError::Transport)) => {
                    let kind = release_kind(dispatched.load(Ordering::SeqCst));
                    self.admission.release(slot, kind);
                    Err(failed("transport"))
                }
                Ok(Err(other)) => {
                    // The response head was received: the request
                    // outcome is known, so the slot is released as
                    // completed even when the answer is rejected.
                    self.admission.release(slot, ReleaseKind::Completed);
                    Err(match other {
                        ExchangeError::RedirectBlocked => failed("redirect_blocked"),
                        ExchangeError::RateLimited => failed("rate_limited"),
                        ExchangeError::HttpStatus(code) => failed(&format!("http_{code}")),
                        ExchangeError::ResponseTooLarge => failed("response_too_large"),
                        ExchangeError::Rejected(reason) => ObservationStatus::Rejected { reason },
                        ExchangeError::Transport => unreachable!("handled in the arm above"),
                    })
                }
                Ok(Ok(answer)) => {
                    self.admission.release(slot, ReleaseKind::Completed);
                    Ok(answer)
                }
            }
        };
        outcome
    }

    async fn exchange(
        &self,
        target: &ChatTarget,
        addresses: &[std::net::SocketAddr],
        body: Bytes,
        authorization: Option<HeaderValue>,
        dispatched: &Arc<AtomicBool>,
        question: &ShadowQuestion,
    ) -> Result<ShadowAnswer, ExchangeError> {
        let mut connected = None;
        for address in addresses {
            if let Ok(stream) = TcpStream::connect(address).await {
                connected = Some(stream);
                break;
            }
        }
        let stream = connected.ok_or(ExchangeError::Transport)?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| ExchangeError::Transport)?;
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let builder = hyper::Request::builder()
            .method("POST")
            .uri("/v1/systemone")
            .header("Host", target.authority.clone())
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("Connection", "close");
        let builder = match authorization {
            Some(header) => builder.header(hyper::header::AUTHORIZATION, header),
            None => builder,
        };
        let http_request = builder
            .body(Full::new(body))
            .expect("static request template");

        dispatched.store(true, Ordering::SeqCst);
        let response = sender
            .send_request(http_request)
            .await
            .map_err(|_| ExchangeError::Transport)?;

        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            // The Location target is never contacted; the body is
            // dropped unread.
            return Err(ExchangeError::RedirectBlocked);
        }
        if status == 429 {
            return Err(ExchangeError::RateLimited);
        }
        if !(200..300).contains(&status) {
            return Err(ExchangeError::HttpStatus(status));
        }

        let mut body = response.into_body();
        let mut collected: Vec<u8> = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| ExchangeError::Transport)?;
            let chunk = frame.into_data().map_err(|_| ExchangeError::Transport)?;
            if collected.len() + chunk.len() > self.config.response_ceiling_bytes {
                return Err(ExchangeError::ResponseTooLarge);
            }
            collected.extend_from_slice(&chunk);
        }
        wire::parse_answer(&collected, question).map_err(ExchangeError::Rejected)
    }

    fn emit(&self, record: &ShadowObservationRecord, started: Instant) {
        let end_unix_ms = unix_ms();
        let duration = started.elapsed();
        let start_unix_ms = end_unix_ms.saturating_sub(duration.as_millis() as i64);
        let mut attributes = BTreeMap::new();
        attributes.insert(
            attributes::OPERATION_NAME.to_owned(),
            "systemone".to_owned(),
        );
        attributes.insert(
            attributes::PROVIDER_NAME.to_owned(),
            LAYA_PROTOCOL.to_owned(),
        );
        attributes.insert(attributes::REQUEST_MODEL.to_owned(), record.model.clone());
        attributes.insert(
            attributes::FLUZO_INFERENCE_PURPOSE.to_owned(),
            "shadow_observation".to_owned(),
        );
        attributes.insert("fluzo.shadow.status".to_owned(), record.status.code());
        let status = if record.status == ObservationStatus::Completed {
            SpanStatus::Ok
        } else {
            SpanStatus::Error {
                kind: record.status.code(),
            }
        };
        let _ = self.hub.publish(DiagnosticRecord::new(
            SignalKind::Span,
            end_unix_ms,
            self.resource.clone(),
            None,
            DiagnosticPayload::Span(SpanRecord {
                name: gen_ai_span_name("systemone", &record.model),
                kind: SpanKind::Client,
                start_unix_ms,
                end_unix_ms,
                status,
                attributes: attributes.clone(),
                // Shadow spans stay unsampled, like the chat
                // gateway's shadow-purpose requests.
                sampled: false,
            }),
        ));
        let _ = self.hub.publish(DiagnosticRecord::new(
            SignalKind::Metric,
            end_unix_ms,
            self.resource.clone(),
            None,
            DiagnosticPayload::Metric(gateway::histogram(
                instruments::OPERATION_DURATION,
                SECONDS_UNIT,
                duration.as_secs_f64(),
                start_unix_ms,
                end_unix_ms,
                attributes,
            )),
        ));
    }
}

impl DecisionObserver for LayaShadowObserver {
    fn observe(
        self: Arc<Self>,
        request: ShadowRequest,
        cancel: watch::Receiver<bool>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ShadowObservationRecord> + Send + 'static>,
    > {
        Box::pin(async move { self.run(request, cancel).await })
    }
}

fn answer_action(answer: &ShadowAnswer) -> String {
    match answer {
        ShadowAnswer::Choice { action, .. }
        | ShadowAnswer::Noul { action, .. }
        | ShadowAnswer::Score { action, .. } => action.recommended.clone(),
    }
}

fn release_kind(dispatched: bool) -> ReleaseKind {
    if dispatched {
        ReleaseKind::Uncertain
    } else {
        ReleaseKind::Completed
    }
}

fn failed(code: &str) -> ObservationStatus {
    ObservationStatus::Failed {
        code: code.to_owned(),
    }
}

fn credential_code(error: gateway::GatewayError) -> String {
    match error {
        gateway::GatewayError::CredentialInvalid => "credential_invalid",
        gateway::GatewayError::CredentialUnavailable => "credential_unavailable",
        _ => "credential_error",
    }
    .to_owned()
}

fn skip_code(reason: ShadowSkipReason) -> &'static str {
    match reason {
        ShadowSkipReason::ForegroundQueued => "foreground_queued",
        ShadowSkipReason::NoShadowAllowance => "no_shadow_allowance",
        ShadowSkipReason::ModelAtLimit => "model_at_limit",
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

enum ExchangeError {
    Transport,
    RedirectBlocked,
    RateLimited,
    HttpStatus(u16),
    ResponseTooLarge,
    Rejected(wire::AnswerRejection),
}

#[cfg(test)]
#[path = "observer_tests.rs"]
mod observer_tests;
