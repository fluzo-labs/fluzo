//! AGT-01 provider gateway: the gated Chat Completions call path.
//!
//! Every generative request runs the same ordered contract (PRD 42.1,
//! 43.1/43.3, Architecture A04/A08/A09):
//!
//! 1. Strict endpoint parsing (http only, no query/userinfo/fragment).
//! 2. DNS resolution and the #110 address classification, then the
//!    consent gate: loopback is allowed; any other class needs an
//!    explicit prior acceptance for that (endpoint, class) pair, and
//!    a class change after acceptance is a denial. Zero sockets are
//!    opened before the gate says yes.
//! 3. Credential-reference resolution: only the named environment
//!    variable's value reaches the `Authorization` header (marked
//!    sensitive); the value never enters errors, diagnostics or any
//!    assembled value.
//! 4. Admission (#112): every attempt, including retries, enters
//!    `AdmissionController::try_admit`. Queued waits are async,
//!    bounded by the queue/task deadline and cancellable; the slot is
//!    held until the stream fully completes, not until headers.
//! 5. HTTP with typed status handling: 3xx is `RedirectBlocked` and
//!    the `Location` target is never contacted; 429 is `RateLimited`
//!    with zero automatic retry and `Retry-After` is never an input;
//!    only Transient (5xx / connection failure before any delta) may
//!    retry, bounded by `retries`, and each retry re-enters admission.
//! 6. Streaming through the pure SSE framer and Chat assembler: tool
//!    calls are JSON-validated at finish before they are exposed
//!    (AC1). A break after any surfaced delta is `PartialStream` and
//!    leaves the admission slot `Uncertain`; it is never retried.
//! 7. One client span plus duration / time-to-first-chunk histograms
//!    and known-usage counters per logical operation, published
//!    through the existing `DiagnosticHub` with the pinned GenAI
//!    vocabulary. Unknown usage is omitted, never zeroed, and no
//!    content attribute is ever produced.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fluzo_core::admission::{
    BudgetConsumption, ProviderRejection, RequestClass, TaskBudgets, classify_status,
};
use fluzo_core::model_discovery::AddressClass;
use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, GenAiUsage, MetricAggregation, MetricKind, MetricSnapshot,
    MetricTemporality, ProviderUsageReport, ResourceIdentity, SECONDS_UNIT, SignalKind, SpanKind,
    SpanRecord, SpanStatus, TOKEN_UNIT, UsageNormalizer, attributes, gen_ai_span_name, instruments,
};
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::HeaderValue;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::admission::{
    Admission, AdmissionController, AdmissionDenied, ModelId, PoolId, ReleaseKind, SlotId, Ticket,
};
use crate::model_discovery::classify;
use crate::telemetry::DiagnosticHub;

use super::chat::{
    AssembledResponse, ChatError, ChatStreamAssembler, FeedOutcome, build_chat_request,
};
use super::sse::{SseError, SseEvent, SseParser};

/// Provider identity carried by the pinned `gen_ai.provider.name`
/// attribute for this adapter.
pub const PROVIDER_PROTOCOL: &str = "openai_compatible";

/// Hard ceiling on one framed SSE event. The assembled-response
/// ceiling in `chat` bounds the whole stream; this bounds a single
/// hostile frame.
pub const SSE_EVENT_CEILING: usize = 1024 * 1024;

/// Poll interval while a request waits on its admission ticket. The
/// controller is non-blocking, so waiting is polled, bounded by the
/// ticket deadline and interruptible by cancellation.
const ADMISSION_POLL: Duration = Duration::from_millis(5);

/// Histogram bounds shared by the duration instruments (seconds).
const DURATION_BOUNDS: [f64; 8] = [0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0];

/// Typed consent outcome for a resolved address class, mirroring the
/// #110 rules: `Required` means no acceptance exists for this class,
/// `Denied` means the class changed after an acceptance (or policy
/// refuses outright).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsentError {
    Required { class: AddressClass },
    Denied { class: AddressClass },
}

/// Which gateway deadline fired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutKind {
    Connect,
    FirstOutput,
    IdleStream,
    Request,
}

/// Typed gateway failure. No variant carries a credential value, a
/// prompt, a response body or a raw endpoint userinfo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GatewayError {
    InvalidEndpoint,
    Consent(ConsentError),
    CredentialUnavailable,
    CredentialInvalid,
    Admission(AdmissionDenied),
    RedirectBlocked,
    Rejected {
        status: u16,
        rejection: ProviderRejection,
    },
    Timeout(TimeoutKind),
    Stream(ChatError),
    Framing(SseError),
    StreamEnded,
    Transport,
    Cancelled,
    /// The stream broke after deltas were already surfaced. The text
    /// received so far is returned for display, but the response is
    /// a failure: partial tool-call buffers are never exposed, and
    /// the admission slot was left `Uncertain`. Never retried.
    PartialStream(Box<PartialStreamInfo>),
}

/// Display-only payload of a broken stream. Tool-call buffers are
/// deliberately absent: they are never dispatchable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialStreamInfo {
    pub text: String,
    pub finish_reasons: Vec<String>,
    pub usage: Option<GenAiUsage>,
}

impl GatewayError {
    /// Stable machine-readable code for `error.type` and headless
    /// results.
    pub fn code(&self) -> String {
        match self {
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::Consent(ConsentError::Required { .. }) => "consent_required",
            Self::Consent(ConsentError::Denied { .. }) => "consent_denied",
            Self::CredentialUnavailable => "credential_unavailable",
            Self::CredentialInvalid => "credential_invalid",
            Self::Admission(denied) => return denied.code().to_owned(),
            Self::RedirectBlocked => "redirect_blocked",
            Self::Rejected { rejection, .. } => match rejection {
                ProviderRejection::RateLimited => "rate_limited",
                ProviderRejection::Transient => "transient_rejected",
                ProviderRejection::Fatal => "fatal_rejected",
            },
            Self::Timeout(kind) => match kind {
                TimeoutKind::Connect => "timeout_connect",
                TimeoutKind::FirstOutput => "timeout_first_output",
                TimeoutKind::IdleStream => "timeout_idle_stream",
                TimeoutKind::Request => "timeout_request",
            },
            Self::Stream(ChatError::MalformedFrame) => "stream_malformed_frame",
            Self::Stream(ChatError::InvalidToolCall { .. }) => "stream_invalid_tool_call",
            Self::Stream(ChatError::ResponseTooLarge) => "stream_response_too_large",
            Self::Framing(SseError::BufferExceeded) => "framing_buffer_exceeded",
            Self::Framing(SseError::InvalidUtf8) => "framing_invalid_utf8",
            Self::StreamEnded => "stream_ended",
            Self::Transport => "transport",
            Self::Cancelled => "cancelled",
            Self::PartialStream { .. } => "partial_stream",
        }
        .to_owned()
    }
}

/// Consent decision source. The gateway asks once per call, after
/// resolution and before any socket is opened.
pub trait ConsentGate: Send + Sync {
    fn allow(&self, endpoint: &str, class: AddressClass) -> Result<(), ConsentError>;
}

/// Default gate: loopback only. Everything else reports `Required`
/// (no acceptance exists) — the caller must surface the need instead
/// of silently connecting.
pub struct LoopbackOnlyConsent;

impl ConsentGate for LoopbackOnlyConsent {
    fn allow(&self, _endpoint: &str, class: AddressClass) -> Result<(), ConsentError> {
        match class {
            AddressClass::Loopback => Ok(()),
            class => Err(ConsentError::Required { class }),
        }
    }
}

/// Per-run accepted (endpoint, class) pairs, mirroring the #110
/// consent bookkeeping: an acceptance is valid only for the exact
/// class that was shown to the operator, and a class change after
/// acceptance is a denial rather than a re-prompt surprise.
#[derive(Default)]
pub struct AcceptedEndpoints {
    accepted: Mutex<BTreeSet<(String, AddressClass)>>,
}

impl AcceptedEndpoints {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&self, endpoint: String, class: AddressClass) {
        if class == AddressClass::Unresolved {
            return;
        }
        self.lock().insert((endpoint, class));
    }

    /// Drop every acceptance for an endpoint (e.g. after the operator
    /// withdraws consent).
    pub fn revoke(&self, endpoint: &str) {
        self.lock().retain(|(accepted, _)| accepted != endpoint);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeSet<(String, AddressClass)>> {
        self.accepted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ConsentGate for AcceptedEndpoints {
    fn allow(&self, endpoint: &str, class: AddressClass) -> Result<(), ConsentError> {
        if class == AddressClass::Loopback {
            return Ok(());
        }
        if class == AddressClass::Unresolved {
            return Err(ConsentError::Denied { class });
        }
        let accepted = self.lock();
        if accepted.contains(&(endpoint.to_owned(), class)) {
            return Ok(());
        }
        if accepted.iter().any(|(accepted, _)| accepted == endpoint) {
            // Accepted under a different address class: fail closed.
            return Err(ConsentError::Denied { class });
        }
        Err(ConsentError::Required { class })
    }
}

/// Credential-reference resolver. The gateway only ever sees the
/// value at header-build time; injectable so tests can use a scripted
/// secret without touching the process environment.
pub trait CredentialSource: Send + Sync {
    fn resolve(&self, name: &str) -> Option<String>;
}

/// Production source: the process environment.
pub struct SystemEnv;

impl CredentialSource for SystemEnv {
    fn resolve(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Static gateway configuration, mapped from the typed settings.
pub struct GatewayConfig {
    /// Base endpoint (`http://host:port` or `http://host:port/v1`).
    pub endpoint: String,
    /// Name of the credential environment variable, never its value.
    pub api_key_env: Option<String>,
    pub pool: PoolId,
    pub capacity: ModelId,
    /// Additional safe transient retries (never quota recovery).
    pub retries: u32,
    pub connect_timeout: Duration,
    pub first_output_timeout: Duration,
    pub idle_stream_timeout: Duration,
    pub request_timeout: Duration,
}

/// One logical generative operation.
pub struct ChatRequest {
    pub purpose: RequestClass,
    pub model: String,
    pub messages: Vec<Value>,
    pub tools: Option<Vec<Value>>,
    pub max_output_tokens: u64,
    /// Conservative input estimate owned by the caller (admission).
    pub input_estimate_tokens: u64,
    pub remaining_task_seconds: u64,
    pub budgets: Option<TaskBudgets>,
    pub consumed: Option<BudgetConsumption>,
}

/// A fully assembled, dispatch-eligible success.
#[derive(Debug)]
pub struct GatewayResponse {
    pub assembled: AssembledResponse,
    /// Normalized known usage for the logical operation (final
    /// response identity). Empty means nothing was reported.
    pub usage: GenAiUsage,
    /// Total attempts including transient retries.
    pub attempts: u32,
    pub time_to_first_chunk: Option<Duration>,
}

/// Outcome of one physical attempt inside the retry loop.
enum AttemptOutcome {
    Success {
        assembled: AssembledResponse,
        ttfc: Option<Duration>,
    },
    /// Retry-eligible only when no delta was surfaced; the caller
    /// decides with the remaining retry budget.
    Transient(GatewayError),
    /// Deltas were already surfaced when the stream broke.
    Partial {
        text: String,
        finish_reasons: Vec<String>,
        release: ReleaseKind,
    },
    Terminal(GatewayError),
    /// Cancelled after the request was handed to the transport: the
    /// remote outcome is unconfirmed.
    Cancelled,
}

/// The gateway. `Clone`-free by design: it holds the consent gate,
/// the admission controller, the diagnostic hub and the credential
/// source as shared handles, so one instance per configured backend
/// is wired once at startup.
pub struct ProviderGateway {
    config: GatewayConfig,
    admission: Arc<AdmissionController>,
    consent: Arc<dyn ConsentGate>,
    credentials: Arc<dyn CredentialSource>,
    hub: Arc<DiagnosticHub>,
    resource: ResourceIdentity,
    /// Slots handed to us by `admit_next` while waiting on a
    /// different ticket (concurrent calls on one controller).
    handed_off: Mutex<BTreeMap<Ticket, SlotId>>,
    clock: Instant,
}

impl ProviderGateway {
    pub fn new(
        config: GatewayConfig,
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
            handed_off: Mutex::new(BTreeMap::new()),
            clock: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// Run one logical generative operation through the full contract.
    /// `cancel` is a cooperative flag: set it to `true` to abandon
    /// the operation at the next await point.
    pub async fn complete(
        &self,
        request: &ChatRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<GatewayResponse, GatewayError> {
        let started = Instant::now();
        let mut normalizer = UsageNormalizer::default();
        let result = self.run(request, cancel, &mut normalizer).await;
        self.emit(request, &result, &normalizer, started);
        result
    }

    async fn run(
        &self,
        request: &ChatRequest,
        cancel: &mut watch::Receiver<bool>,
        normalizer: &mut UsageNormalizer,
    ) -> Result<GatewayResponse, GatewayError> {
        let target = chat_target(&self.config.endpoint)?;
        let addresses = resolve(&target).await;
        let class = classify(&addresses);
        if class == AddressClass::Unresolved {
            return Err(GatewayError::Transport);
        }
        // Consent before any socket: a denial means zero packets left
        // the process for this operation.
        self.consent
            .allow(&self.config.endpoint, class)
            .map_err(GatewayError::Consent)?;
        let authorization = self.authorization()?;
        let body = Bytes::from(
            build_chat_request(
                &request.model,
                request.messages.clone(),
                request.tools.clone(),
                request.max_output_tokens,
            )
            .to_string(),
        );

        let mut attempts = 0u32;
        loop {
            if *cancel.borrow_and_update() {
                return Err(GatewayError::Cancelled);
            }
            let slot = self.acquire(request, cancel).await?;
            attempts += 1;
            let endpoint = ResolvedTarget {
                target: &target,
                addresses: &addresses,
            };
            match self
                .attempt(
                    &endpoint,
                    &body,
                    authorization.as_ref(),
                    cancel,
                    normalizer,
                    attempts,
                )
                .await
            {
                AttemptOutcome::Success { assembled, ttfc } => {
                    self.admission.release(slot, ReleaseKind::Completed);
                    return Ok(GatewayResponse {
                        assembled,
                        usage: normalizer.known(),
                        attempts,
                        time_to_first_chunk: ttfc,
                    });
                }
                AttemptOutcome::Transient(error) => {
                    // The attempt terminated with its response fully
                    // consumed (or never sent): capacity is provably
                    // free to release.
                    self.admission.release(slot, ReleaseKind::Completed);
                    if attempts <= self.config.retries {
                        continue;
                    }
                    return Err(error);
                }
                AttemptOutcome::Partial {
                    text,
                    finish_reasons,
                    release,
                } => {
                    self.admission.release(slot, release);
                    normalizer.finish_incomplete();
                    let usage = normalizer.known();
                    return Err(GatewayError::PartialStream(Box::new(PartialStreamInfo {
                        text,
                        finish_reasons,
                        usage: if usage.is_empty() { None } else { Some(usage) },
                    })));
                }
                AttemptOutcome::Terminal(error) => {
                    self.admission.release(slot, ReleaseKind::Completed);
                    return Err(error);
                }
                AttemptOutcome::Cancelled => {
                    // The request was dispatched: dropping the
                    // connection is not proof the provider stopped,
                    // so the slot stays uncertain until reconciled.
                    self.admission.release(slot, ReleaseKind::Uncertain);
                    return Err(GatewayError::Cancelled);
                }
            }
        }
    }

    /// Admission with bounded, cancellable queue waiting.
    async fn acquire(
        &self,
        request: &ChatRequest,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<SlotId, GatewayError> {
        let admission_request = crate::admission::AdmissionRequest {
            class: request.purpose,
            pool: self.config.pool.clone(),
            model: self.config.capacity.clone(),
            input_estimate_tokens: request.input_estimate_tokens,
            max_response_output_tokens: request.max_output_tokens,
            remaining_task_seconds: request.remaining_task_seconds,
            budgets: request.budgets,
            consumed: request.consumed,
        };
        match self.admission.try_admit(&admission_request, self.now_ms()) {
            Admission::Admitted(slot) => Ok(slot),
            Admission::Denied(denied) => Err(GatewayError::Admission(denied)),
            Admission::Queued {
                ticket,
                deadline_ms,
                ..
            } => loop {
                if *cancel.borrow_and_update() {
                    self.admission.cancel_queued(ticket);
                    return Err(GatewayError::Cancelled);
                }
                if let Some(slot) = self.take_handed_off(ticket) {
                    return Ok(slot);
                }
                match self.admission.admit_next(&self.config.pool, self.now_ms()) {
                    Some((owned, slot)) if owned == ticket => return Ok(slot),
                    Some((other, slot)) => {
                        // A concurrent call's slot was freed to us by
                        // FIFO; stash it for its owner instead of
                        // stealing it.
                        self.handed_off
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(other, slot);
                    }
                    None => {}
                }
                if self.now_ms() >= deadline_ms {
                    self.admission.cancel_queued(ticket);
                    return Err(GatewayError::Admission(
                        AdmissionDenied::CapacityWaitTimeout,
                    ));
                }
                tokio::select! {
                    _ = cancel.changed() => {}
                    _ = tokio::time::sleep(ADMISSION_POLL) => {}
                }
            },
        }
    }

    fn take_handed_off(&self, ticket: Ticket) -> Option<SlotId> {
        let mut handed = self
            .handed_off
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        handed.remove(&ticket)
    }

    fn authorization(&self) -> Result<Option<HeaderValue>, GatewayError> {
        let Some(name) = self.config.api_key_env.as_deref() else {
            return Ok(None);
        };
        if name.is_empty()
            || name.len() > 128
            || !name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
        {
            return Err(GatewayError::CredentialInvalid);
        }
        let value = self
            .credentials
            .resolve(name)
            .ok_or(GatewayError::CredentialUnavailable)?;
        if value.is_empty()
            || value.len() > 4096
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(GatewayError::CredentialInvalid);
        }
        let mut header = HeaderValue::from_str(&format!("Bearer {value}"))
            .map_err(|_| GatewayError::CredentialInvalid)?;
        header.set_sensitive(true);
        Ok(Some(header))
    }

    /// One physical attempt: connect, send, stream. Never retries;
    /// the caller owns the retry budget.
    async fn attempt(
        &self,
        endpoint: &ResolvedTarget<'_>,
        body: &Bytes,
        authorization: Option<&HeaderValue>,
        cancel: &mut watch::Receiver<bool>,
        normalizer: &mut UsageNormalizer,
        attempt_no: u32,
    ) -> AttemptOutcome {
        let op_start = tokio::time::Instant::now();
        let connect = async {
            let mut connected = None;
            for address in endpoint.addresses {
                if let Ok(stream) = TcpStream::connect(address).await {
                    connected = Some(stream);
                    break;
                }
            }
            let stream = connected.ok_or(())?;
            let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
                .await
                .map_err(|_| ())?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            Ok::<_, ()>(sender)
        };
        let mut sender = match tokio::time::timeout(self.config.connect_timeout, connect).await {
            Err(_) => {
                return AttemptOutcome::Transient(GatewayError::Timeout(TimeoutKind::Connect));
            }
            Ok(Err(())) => return AttemptOutcome::Transient(GatewayError::Transport),
            Ok(Ok(sender)) => sender,
        };

        let request = hyper::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Host", endpoint.target.authority.clone())
            .header("Accept", "text/event-stream")
            .header("Content-Type", "application/json")
            .header("Connection", "close");
        let request = match authorization {
            Some(header) => request.header(hyper::header::AUTHORIZATION, header),
            None => request,
        };
        let request = request
            .body(Full::new(body.clone()))
            .expect("static request template");

        let response = tokio::select! {
            biased;
            _ = cancel.wait_for(|cancelled| *cancelled) => {
                // The request body was already handed to the
                // transport: whether the provider saw it is unknown,
                // so the slot must not be treated as provably free.
                return AttemptOutcome::Cancelled;
            }
            sent = sender.send_request(request) => match sent {
                Ok(response) => response,
                Err(_) => return AttemptOutcome::Transient(GatewayError::Transport),
            }
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            match classify_status(status) {
                ProviderRejection::RateLimited => {
                    return AttemptOutcome::Terminal(GatewayError::Rejected {
                        status,
                        rejection: ProviderRejection::RateLimited,
                    });
                }
                ProviderRejection::Transient => {
                    return AttemptOutcome::Transient(GatewayError::Rejected {
                        status,
                        rejection: ProviderRejection::Transient,
                    });
                }
                ProviderRejection::Fatal => {
                    if (300..400).contains(&status) {
                        // The Location target is never contacted: the
                        // response body is dropped unread.
                        return AttemptOutcome::Terminal(GatewayError::RedirectBlocked);
                    }
                    return AttemptOutcome::Terminal(GatewayError::Rejected {
                        status,
                        rejection: ProviderRejection::Fatal,
                    });
                }
            }
        }

        // Streaming phase.
        let mut parser = SseParser::new(SSE_EVENT_CEILING);
        let mut assembler = ChatStreamAssembler::new();
        let mut body = response.into_body();
        let mut ttfc: Option<Duration> = None;
        let mut deltas_surfaced = false;
        let mut last_event = op_start;
        let request_deadline = op_start + self.config.request_timeout;

        loop {
            let now = tokio::time::Instant::now();
            // Deadline selection: whole request first, then first
            // output while nothing has been seen, then idle stream.
            let (deadline, kind) = if now >= request_deadline {
                (request_deadline, TimeoutKind::Request)
            } else if !deltas_surfaced {
                let first = op_start + self.config.first_output_timeout;
                if first <= last_event + self.config.idle_stream_timeout {
                    (first, TimeoutKind::FirstOutput)
                } else {
                    (
                        last_event + self.config.idle_stream_timeout,
                        TimeoutKind::IdleStream,
                    )
                }
            } else {
                (
                    last_event + self.config.idle_stream_timeout,
                    TimeoutKind::IdleStream,
                )
            };
            if now >= deadline {
                return Self::break_out(deltas_surfaced, &assembler, kind);
            }
            let wait = deadline - now;
            tokio::select! {
                biased;
                _ = cancel.wait_for(|cancelled| *cancelled) => {
                    return AttemptOutcome::Cancelled;
                }
                frame = body.frame() => {
                    match frame {
                        Some(Err(_)) => {
                            if deltas_surfaced {
                                return AttemptOutcome::Partial {
                                    text: assembler.text_so_far(),
                                    finish_reasons: assembler.finish_reasons_so_far(),
                                    release: ReleaseKind::Uncertain,
                                };
                            }
                            return AttemptOutcome::Transient(GatewayError::Transport);
                        }
                        None => {
                            if parser.is_done() {
                                break;
                            }
                            if deltas_surfaced {
                                // Clean transport end without the
                                // sentinel: transmission finished,
                                // so the remote outcome is known,
                                // but the contract was violated.
                                return AttemptOutcome::Partial {
                                    text: assembler.text_so_far(),
                                    finish_reasons: assembler.finish_reasons_so_far(),
                                    release: ReleaseKind::Completed,
                                };
                            }
                            return AttemptOutcome::Terminal(GatewayError::StreamEnded);
                        }
                        Some(Ok(frame)) => {
                            let Some(data) = frame.data_ref() else {
                                continue;
                            };
                            let events = match parser.push(data) {
                                Ok(events) => events,
                                Err(error) => {
                                    return AttemptOutcome::Terminal(GatewayError::Framing(error));
                                }
                            };
                            let mut saw_done = false;
                            for event in events {
                                match event {
                                    SseEvent::Done => {
                                        saw_done = true;
                                        break;
                                    }
                                    SseEvent::Data(payload) => {
                                        last_event = tokio::time::Instant::now();
                                        let outcomes = match assembler.feed(&payload) {
                                            Ok(outcomes) => outcomes,
                                            Err(error) => {
                                                return AttemptOutcome::Terminal(
                                                    GatewayError::Stream(error),
                                                );
                                            }
                                        };
                                        for outcome in outcomes {
                                            match outcome {
                                                FeedOutcome::TextDelta(_)
                                                | FeedOutcome::ToolCallDelta { .. } => {
                                                    if !deltas_surfaced {
                                                        deltas_surfaced = true;
                                                        ttfc =
                                                            Some(last_event.duration_since(op_start));
                                                    }
                                                }
                                                FeedOutcome::Usage(report) => {
                                                    Self::record_usage(
                                                        normalizer,
                                                        report,
                                                        attempt_no,
                                                    );
                                                }
                                                FeedOutcome::Ignored => {}
                                            }
                                        }
                                    }
                                }
                            }
                            if saw_done {
                                break;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(wait) => {
                    // Loop re-evaluates the deadline and classifies
                    // the timeout on the next pass.
                }
            }
        }

        match assembler.finish() {
            Ok(assembled) => AttemptOutcome::Success { assembled, ttfc },
            Err(error) => AttemptOutcome::Terminal(GatewayError::Stream(error)),
        }
    }

    fn break_out(
        deltas_surfaced: bool,
        assembler: &ChatStreamAssembler,
        kind: TimeoutKind,
    ) -> AttemptOutcome {
        if deltas_surfaced {
            // A timeout after deltas is a partial stream: the remote
            // may still be generating, so the slot stays Uncertain
            // and no retry happens.
            AttemptOutcome::Partial {
                text: assembler.text_so_far(),
                finish_reasons: assembler.finish_reasons_so_far(),
                release: ReleaseKind::Uncertain,
            }
        } else {
            // Nothing was received: connection-transient per the
            // contract, retry-eligible within budget.
            AttemptOutcome::Transient(GatewayError::Timeout(kind))
        }
    }

    fn record_usage(
        normalizer: &mut UsageNormalizer,
        report: ProviderUsageReport,
        attempt_no: u32,
    ) {
        if attempt_no == 1 {
            normalizer.report(report);
        } else {
            // Retry consumption keeps the distinct PhysicalRetry
            // identity and is never deduplicated against the logical
            // operation's cumulative stream.
            normalizer.report_retry(report);
        }
    }

    /// Publish the pinned GenAI span and metrics for one logical
    /// operation through the diagnostic hub.
    fn emit(
        &self,
        request: &ChatRequest,
        result: &Result<GatewayResponse, GatewayError>,
        normalizer: &UsageNormalizer,
        started: Instant,
    ) {
        let end_unix_ms = unix_ms();
        let duration = started.elapsed();
        let start_unix_ms = end_unix_ms.saturating_sub(duration.as_millis() as i64);
        let mut attributes = self.base_attributes(request);
        let status = match result {
            Ok(response) => {
                if let Some(id) = &response.assembled.id {
                    attributes.insert(attributes::RESPONSE_ID.to_owned(), id.clone());
                }
                if let Some(model) = &response.assembled.model {
                    attributes.insert(attributes::RESPONSE_MODEL.to_owned(), model.clone());
                }
                if !response.assembled.finish_reasons.is_empty() {
                    attributes.insert(
                        attributes::RESPONSE_FINISH_REASONS.to_owned(),
                        response.assembled.finish_reasons.join(","),
                    );
                }
                SpanStatus::Ok
            }
            Err(error) => {
                let code = error.code();
                attributes.insert(attributes::ERROR_TYPE.to_owned(), code.clone());
                SpanStatus::Error { kind: code }
            }
        };
        let sampled = !matches!(request.purpose, RequestClass::ShadowObservation);
        let _ = self.hub.publish(DiagnosticRecord::new(
            SignalKind::Span,
            end_unix_ms,
            self.resource.clone(),
            None,
            DiagnosticPayload::Span(SpanRecord {
                name: gen_ai_span_name("chat", &request.model),
                kind: SpanKind::Client,
                start_unix_ms,
                end_unix_ms,
                status,
                attributes,
                sampled,
            }),
        ));

        let mut dimensions = BTreeMap::new();
        dimensions.insert(attributes::OPERATION_NAME.to_owned(), "chat".to_owned());
        dimensions.insert(
            attributes::PROVIDER_NAME.to_owned(),
            PROVIDER_PROTOCOL.to_owned(),
        );
        dimensions.insert(attributes::REQUEST_MODEL.to_owned(), request.model.clone());
        if let Err(error) = result {
            dimensions.insert(attributes::ERROR_TYPE.to_owned(), error.code());
        }

        let duration_seconds = duration.as_secs_f64();
        let _ = self.hub.publish(DiagnosticRecord::new(
            SignalKind::Metric,
            end_unix_ms,
            self.resource.clone(),
            None,
            DiagnosticPayload::Metric(histogram(
                instruments::OPERATION_DURATION,
                SECONDS_UNIT,
                duration_seconds,
                start_unix_ms,
                end_unix_ms,
                dimensions.clone(),
            )),
        ));
        if let Ok(response) = result
            && let Some(ttfc) = response.time_to_first_chunk
        {
            let _ = self.hub.publish(DiagnosticRecord::new(
                SignalKind::Metric,
                end_unix_ms,
                self.resource.clone(),
                None,
                DiagnosticPayload::Metric(histogram(
                    instruments::OPERATION_TIME_TO_FIRST_CHUNK,
                    SECONDS_UNIT,
                    ttfc.as_secs_f64(),
                    start_unix_ms,
                    end_unix_ms,
                    dimensions.clone(),
                )),
            ));
        }

        // Known usage counters only: unknown fields are omitted, never
        // written as zero.
        let usage = normalizer.known();
        let counters = [
            (instruments::USAGE_INPUT_TOKENS, usage.input_tokens),
            (instruments::USAGE_OUTPUT_TOKENS, usage.output_tokens),
            (
                instruments::USAGE_CACHE_READ_INPUT_TOKENS,
                usage.cache_read_input_tokens,
            ),
            (
                instruments::USAGE_CACHE_WRITE_INPUT_TOKENS,
                usage.cache_write_input_tokens,
            ),
            (
                instruments::USAGE_REASONING_OUTPUT_TOKENS,
                usage.reasoning_output_tokens,
            ),
        ];
        for (name, value) in counters {
            let Some(value) = value else { continue };
            let _ = self.hub.publish(DiagnosticRecord::new(
                SignalKind::Metric,
                end_unix_ms,
                self.resource.clone(),
                None,
                DiagnosticPayload::Metric(MetricSnapshot {
                    name: name.to_owned(),
                    unit: TOKEN_UNIT.to_owned(),
                    kind: MetricKind::Counter,
                    temporality: MetricTemporality::Delta,
                    window_start_ms: start_unix_ms,
                    window_end_ms: end_unix_ms,
                    dimensions: dimensions.clone(),
                    aggregation: MetricAggregation::Sum {
                        value: value as f64,
                    },
                }),
            ));
        }
    }

    fn base_attributes(&self, request: &ChatRequest) -> BTreeMap<String, String> {
        let mut attributes = BTreeMap::new();
        attributes.insert(attributes::OPERATION_NAME.to_owned(), "chat".to_owned());
        attributes.insert(
            attributes::PROVIDER_NAME.to_owned(),
            PROVIDER_PROTOCOL.to_owned(),
        );
        attributes.insert(attributes::REQUEST_MODEL.to_owned(), request.model.clone());
        attributes.insert(attributes::REQUEST_STREAM.to_owned(), "true".to_owned());
        attributes.insert(
            attributes::REQUEST_MAX_TOKENS.to_owned(),
            request.max_output_tokens.to_string(),
        );
        attributes.insert(
            attributes::FLUZO_INFERENCE_PURPOSE.to_owned(),
            purpose_code(request.purpose).to_owned(),
        );
        attributes
    }
}

fn purpose_code(purpose: RequestClass) -> &'static str {
    match purpose {
        RequestClass::AgentTurn => "agent_turn",
        RequestClass::Compaction => "compaction",
        RequestClass::SetupProbe => "setup_probe",
        RequestClass::LiveTest => "live_test",
        RequestClass::ShadowObservation => "shadow_observation",
    }
}

fn histogram(
    name: &str,
    unit: &str,
    value: f64,
    window_start_ms: i64,
    window_end_ms: i64,
    dimensions: BTreeMap<String, String>,
) -> MetricSnapshot {
    let mut bucket_counts = vec![0u64; DURATION_BOUNDS.len() + 1];
    let index = DURATION_BOUNDS
        .iter()
        .position(|bound| value <= *bound)
        .unwrap_or(DURATION_BOUNDS.len());
    bucket_counts[index] = 1;
    MetricSnapshot {
        name: name.to_owned(),
        unit: unit.to_owned(),
        kind: MetricKind::Histogram,
        temporality: MetricTemporality::Delta,
        window_start_ms,
        window_end_ms,
        dimensions,
        aggregation: MetricAggregation::Histogram {
            count: 1,
            sum: value,
            bounds: DURATION_BOUNDS.to_vec(),
            bucket_counts,
        },
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// A resolved endpoint: the parsed target plus its current addresses.
struct ResolvedTarget<'a> {
    target: &'a ChatTarget,
    addresses: &'a [SocketAddr],
}

/// Strict Chat Completions endpoint parse, same discipline as the #110
/// discovery endpoint: http only, no query, no userinfo, no percent
/// escapes, non-zero port. The path is fixed to the Chat Completions
/// resource.
struct ChatTarget {
    host: String,
    port: u16,
    authority: String,
}

fn chat_target(value: &str) -> Result<ChatTarget, GatewayError> {
    if value.len() > 2048 || !value.is_ascii() || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(GatewayError::InvalidEndpoint);
    }
    let uri: hyper::Uri = value.parse().map_err(|_| GatewayError::InvalidEndpoint)?;
    if uri.scheme_str() != Some("http") || uri.query().is_some() || value.contains(['#', '@', '%'])
    {
        return Err(GatewayError::InvalidEndpoint);
    }
    let host = uri.host().ok_or(GatewayError::InvalidEndpoint)?;
    if host.trim_matches(['[', ']']).is_empty() || uri.port_u16() == Some(0) {
        return Err(GatewayError::InvalidEndpoint);
    }
    let base = uri.path().trim_end_matches('/');
    if !base.is_empty() && base != "/v1" {
        return Err(GatewayError::InvalidEndpoint);
    }
    Ok(ChatTarget {
        host: host.trim_matches(['[', ']']).to_owned(),
        port: uri.port_u16().unwrap_or(80),
        authority: uri
            .authority()
            .ok_or(GatewayError::InvalidEndpoint)?
            .to_string(),
    })
}

async fn resolve(target: &ChatTarget) -> Vec<SocketAddr> {
    let mut addresses: Vec<SocketAddr> =
        tokio::net::lookup_host((target.host.as_str(), target.port))
            .await
            .map(|resolved| resolved.collect())
            .unwrap_or_default();
    addresses.sort();
    addresses
}

#[cfg(test)]
#[path = "gateway_tests.rs"]
mod gateway_tests;
