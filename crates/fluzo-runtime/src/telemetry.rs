//! OBS-01 diagnostic hub: privacy-redacted, bounded, non-blocking
//! fan-out to local diagnostic sinks.
//!
//! Every sink gets its own bounded queue (record count AND bytes) and
//! worker thread, so one slow, saturated or failing sink can never
//! stall the producer, the TUI input path or another sink. Redaction is
//! applied once at the capture boundary before fan-out: sinks only ever
//! see redacted records. Refusals are counted per sink with a reason
//! and surfaced through the health projection; they never drop a
//! required SQLite audit record, because audit records never travel
//! through this hub at all.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fluzo_core::telemetry::{DiagnosticPayload, DiagnosticRecord};

const REDACTED_MARKER: &str = "[REDACTED]";

#[derive(Clone, Debug)]
pub struct HubLimits {
    /// Per-sink queue bound in records.
    pub queue_records: usize,
    /// Per-sink queue bound in bytes.
    pub queue_bytes: usize,
    /// Per-record serialized size bound.
    pub max_record_bytes: usize,
}

impl Default for HubLimits {
    fn default() -> Self {
        Self {
            queue_records: 1024,
            queue_bytes: 4 * 1024 * 1024,
            max_record_bytes: 256 * 1024,
        }
    }
}

/// Capture-boundary redaction policy. Known credential-shaped keys are
/// excluded structurally; caller-supplied secret values are scrubbed
/// from text fields. Redaction cannot prove that arbitrary prose holds
/// no unknown secret; it is one layered control, not a guarantee.
#[derive(Clone, Debug)]
pub struct RedactionPolicy {
    /// Lowercase substrings that mark an attribute key as credential-bearing.
    pub forbidden_key_substrings: Vec<String>,
    /// Exact keys exempt from the forbidden-substring rule. Membership is
    /// an exact match, never a substring match, so a key cannot borrow its
    /// way onto the list. This exists because the pinned GenAI vocabulary
    /// legitimately contains credential-shaped substrings:
    /// `gen_ai.token.modality` and every `gen_ai.usage.*_tokens`
    /// attribute would otherwise be destroyed by the broad `token` rule,
    /// silently erasing the token accounting the contract requires.
    pub known_safe_keys: Vec<String>,
    /// Known secret values to scrub from message and attribute text.
    pub secret_values: Vec<String>,
}

/// Pinned standard attributes that contain a credential-shaped substring
/// but are never credentials. Derived from the core vocabulary so the
/// exemption cannot drift from the conventions revision.
fn pinned_safe_genai_keys() -> Vec<String> {
    [
        fluzo_core::telemetry::attributes::TOKEN_MODALITY,
        fluzo_core::telemetry::attributes::USAGE_INPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_OUTPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_CACHE_READ_INPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_CACHE_WRITE_INPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_REASONING_OUTPUT_TOKENS,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl Default for RedactionPolicy {
    fn default() -> Self {
        Self {
            forbidden_key_substrings: [
                "authorization",
                "token",
                "secret",
                "api_key",
                "apikey",
                "password",
                "cookie",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            known_safe_keys: pinned_safe_genai_keys(),
            secret_values: Vec::new(),
        }
    }
}

fn redact_map(
    map: &mut BTreeMap<String, String>,
    forbidden: &[String],
    safe: &[String],
    secrets: &[String],
) -> bool {
    let mut changed = false;
    for (key, value) in map.iter_mut() {
        let lower = key.to_lowercase();
        let pinned_safe = safe.iter().any(|allowed| allowed == key);
        if !pinned_safe
            && forbidden
                .iter()
                .any(|needle| lower.contains(needle.as_str()))
        {
            if value != REDACTED_MARKER {
                *value = REDACTED_MARKER.to_string();
                changed = true;
            }
            continue;
        }
        for secret in secrets {
            if !secret.is_empty() && value.contains(secret.as_str()) {
                *value = value.replace(secret.as_str(), REDACTED_MARKER);
                changed = true;
            }
        }
    }
    changed
}

/// Redacts a record in place before fan-out. Returns whether anything
/// changed (useful for tests; callers must not branch policy on it).
pub fn redact_record(record: &mut DiagnosticRecord, policy: &RedactionPolicy) -> bool {
    let mut changed = false;
    match &mut record.payload {
        DiagnosticPayload::Log(log) => {
            for secret in &policy.secret_values {
                if !secret.is_empty() && log.message.contains(secret.as_str()) {
                    log.message = log.message.replace(secret.as_str(), REDACTED_MARKER);
                    changed = true;
                }
            }
            changed |= redact_map(
                &mut log.attributes,
                &policy.forbidden_key_substrings,
                &policy.known_safe_keys,
                &policy.secret_values,
            );
        }
        DiagnosticPayload::Span(span) => {
            changed |= redact_map(
                &mut span.attributes,
                &policy.forbidden_key_substrings,
                &policy.known_safe_keys,
                &policy.secret_values,
            );
        }
        DiagnosticPayload::Metric(metric) => {
            changed |= redact_map(
                &mut metric.dimensions,
                &policy.forbidden_key_substrings,
                &policy.known_safe_keys,
                &policy.secret_values,
            );
        }
    }
    changed
}

/// A sink consumes batches of already-serialized, already-redacted JSON
/// lines away from the producer. Errors are categories, not raw
/// strings that could echo content back through the pipeline.
pub trait DiagnosticSink: Send + 'static {
    fn name(&self) -> String;
    fn write_batch(&mut self, lines: &[String]) -> Result<(), String>;
    fn finish(&mut self) {}
}

#[derive(Clone, Debug, Default)]
pub struct SinkHealth {
    pub name: String,
    pub queued_records: usize,
    pub queued_bytes: usize,
    pub capacity_records: usize,
    pub capacity_bytes: usize,
    pub dropped: BTreeMap<String, u64>,
    pub last_error: Option<String>,
    pub degraded: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PublishOutcome {
    pub delivered: usize,
    pub dropped: usize,
    pub drop_reason: Option<String>,
}

struct SinkStats {
    queued_records: AtomicUsize,
    queued_bytes: AtomicUsize,
    dropped: Mutex<BTreeMap<String, u64>>,
    last_error: Mutex<Option<String>>,
    degraded: AtomicBool,
}

impl SinkStats {
    fn new() -> Self {
        Self {
            queued_records: AtomicUsize::new(0),
            queued_bytes: AtomicUsize::new(0),
            dropped: Mutex::new(BTreeMap::new()),
            last_error: Mutex::new(None),
            degraded: AtomicBool::new(false),
        }
    }

    fn count_drop(&self, reason: &str) {
        *self
            .dropped
            .lock()
            .unwrap()
            .entry(reason.to_string())
            .or_insert(0) += 1;
    }
}

struct SinkWorker {
    name: String,
    tx: Mutex<Option<mpsc::SyncSender<String>>>,
    stats: Arc<SinkStats>,
    done: Arc<Mutex<mpsc::Receiver<()>>>,
    handle: Option<JoinHandle<()>>,
}

pub struct DiagnosticHub {
    sinks: Vec<SinkWorker>,
    limits: HubLimits,
    policy: RedactionPolicy,
    stopped: Arc<AtomicBool>,
}

impl DiagnosticHub {
    pub fn new(limits: HubLimits, policy: RedactionPolicy) -> Self {
        Self {
            sinks: Vec::new(),
            limits,
            policy,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Registers a sink with its own bounded queue and worker thread.
    pub fn add_sink(&mut self, mut sink: Box<dyn DiagnosticSink>) {
        let name = sink.name();
        let stats = Arc::new(SinkStats::new());
        let (tx, rx) = mpsc::sync_channel::<String>(self.limits.queue_records);
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let worker_stats = Arc::clone(&stats);
        let handle = std::thread::Builder::new()
            .name(format!("fluzo-telemetry-{name}"))
            .spawn(move || {
                let mut batch: Vec<String> = Vec::new();
                let mut batch_bytes = 0usize;
                loop {
                    match rx.recv() {
                        Ok(line) => {
                            batch_bytes += line.len();
                            batch.push(line);
                            while batch.len() < 64 {
                                match rx.try_recv() {
                                    Ok(more) => {
                                        batch_bytes += more.len();
                                        batch.push(more);
                                    }
                                    Err(mpsc::TryRecvError::Empty) => break,
                                    Err(mpsc::TryRecvError::Disconnected) => break,
                                }
                            }
                            if let Err(error) = sink.write_batch(&batch) {
                                // Failure is recorded, never re-published:
                                // no recursive exporter-failure storm.
                                worker_stats.degraded.store(true, Ordering::Relaxed);
                                *worker_stats.last_error.lock().unwrap() = Some(error);
                                for _ in &batch {
                                    worker_stats.count_drop("write_failed");
                                }
                            }
                            worker_stats
                                .queued_records
                                .fetch_sub(batch.len(), Ordering::Relaxed);
                            worker_stats
                                .queued_bytes
                                .fetch_sub(batch_bytes, Ordering::Relaxed);
                            batch.clear();
                            batch_bytes = 0;
                        }
                        Err(mpsc::RecvError) => {
                            if !batch.is_empty() {
                                if sink.write_batch(&batch).is_err() {
                                    worker_stats.degraded.store(true, Ordering::Relaxed);
                                    for _ in &batch {
                                        worker_stats.count_drop("write_failed");
                                    }
                                }
                                worker_stats
                                    .queued_records
                                    .fetch_sub(batch.len(), Ordering::Relaxed);
                                worker_stats
                                    .queued_bytes
                                    .fetch_sub(batch_bytes, Ordering::Relaxed);
                                batch.clear();
                            }
                            break;
                        }
                    }
                }
                sink.finish();
                let _ = done_tx.send(());
            })
            .expect("spawn telemetry sink worker");
        self.sinks.push(SinkWorker {
            name,
            tx: Mutex::new(Some(tx)),
            stats,
            done: Arc::new(Mutex::new(done_rx)),
            handle: Some(handle),
        });
    }

    /// Non-blocking publish. Redacts at the capture boundary, enforces
    /// the per-record size bound and enqueues independently per sink.
    /// Never blocks the caller on sink work.
    pub fn publish(&self, mut record: DiagnosticRecord) -> PublishOutcome {
        if self.stopped.load(Ordering::Relaxed) {
            return PublishOutcome {
                delivered: 0,
                dropped: 1,
                drop_reason: Some("shutdown_stopped".into()),
            };
        }
        redact_record(&mut record, &self.policy);
        let line = match serde_json::to_string(&record) {
            Ok(line) => line,
            Err(_) => {
                for sink in &self.sinks {
                    sink.stats.count_drop("serialize_failed");
                }
                return PublishOutcome {
                    delivered: 0,
                    dropped: 1,
                    drop_reason: Some("serialize_failed".into()),
                };
            }
        };
        if line.len() > self.limits.max_record_bytes {
            for sink in &self.sinks {
                sink.stats.count_drop("record_too_large");
            }
            return PublishOutcome {
                delivered: 0,
                dropped: 1,
                drop_reason: Some("record_too_large".into()),
            };
        }
        let mut outcome = PublishOutcome::default();
        for sink in &self.sinks {
            if sink.stats.queued_records.load(Ordering::Relaxed) >= self.limits.queue_records
                || sink.stats.queued_bytes.load(Ordering::Relaxed) >= self.limits.queue_bytes
            {
                sink.stats.count_drop("queue_full");
                outcome.dropped += 1;
                outcome.drop_reason = Some("queue_full".into());
                continue;
            }
            let mut guard = sink.tx.lock().unwrap();
            match guard.as_mut() {
                Some(tx) => match tx.try_send(line.clone()) {
                    Ok(()) => {
                        sink.stats.queued_records.fetch_add(1, Ordering::Relaxed);
                        sink.stats
                            .queued_bytes
                            .fetch_add(line.len(), Ordering::Relaxed);
                        outcome.delivered += 1;
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        sink.stats.count_drop("queue_full");
                        outcome.dropped += 1;
                        outcome.drop_reason = Some("queue_full".into());
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        sink.stats.count_drop("shutdown_stopped");
                        outcome.dropped += 1;
                        outcome.drop_reason = Some("shutdown_stopped".into());
                    }
                },
                None => {
                    sink.stats.count_drop("shutdown_stopped");
                    outcome.dropped += 1;
                    outcome.drop_reason = Some("shutdown_stopped".into());
                }
            }
        }
        outcome
    }

    pub fn health(&self) -> Vec<SinkHealth> {
        self.sinks
            .iter()
            .map(|sink| SinkHealth {
                name: sink.name.clone(),
                queued_records: sink.stats.queued_records.load(Ordering::Relaxed),
                queued_bytes: sink.stats.queued_bytes.load(Ordering::Relaxed),
                capacity_records: self.limits.queue_records,
                capacity_bytes: self.limits.queue_bytes,
                dropped: sink.stats.dropped.lock().unwrap().clone(),
                last_error: sink.stats.last_error.lock().unwrap().clone(),
                degraded: sink.stats.degraded.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Stops admission immediately and drains sink workers within the
    /// deadline. Undrained records are reported as shutdown drops; the
    /// report is in-memory only and never claimed durable.
    pub fn shutdown(&mut self, deadline: Duration) -> ShutdownReport {
        self.stopped.store(true, Ordering::Relaxed);
        for sink in &mut self.sinks {
            // Dropping the sender lets the worker finish its queue and exit.
            let _ = sink.tx.lock().unwrap().take();
        }
        let start = Instant::now();
        let mut report = ShutdownReport::default();
        for sink in &mut self.sinks {
            let remaining = deadline.saturating_sub(start.elapsed());
            let drained = sink.done.lock().unwrap().recv_timeout(remaining).is_ok();
            if drained {
                if let Some(handle) = sink.handle.take() {
                    let _ = handle.join();
                }
                report.drained_sinks.push(sink.name.clone());
            } else {
                // Deadline expired: report the gap and detach instead of
                // blocking process cancellation on a stuck sink.
                if let Some(handle) = sink.handle.take() {
                    std::mem::drop(handle);
                }
                report.incomplete_flushes.push((
                    sink.name.clone(),
                    sink.stats.queued_records.load(Ordering::Relaxed),
                ));
            }
        }
        report
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShutdownReport {
    pub drained_sinks: Vec<String>,
    /// (sink name, records still queued when the deadline expired).
    pub incomplete_flushes: Vec<(String, usize)>,
}

/// JSONL archive sink over the bounded local archive.
pub struct JsonlArchiveSink {
    archive: crate::telemetry_archive::JsonlArchive,
}

impl JsonlArchiveSink {
    pub fn new(archive: crate::telemetry_archive::JsonlArchive) -> Self {
        Self { archive }
    }
}

impl DiagnosticSink for JsonlArchiveSink {
    fn name(&self) -> String {
        "jsonl-archive".into()
    }

    fn write_batch(&mut self, lines: &[String]) -> Result<(), String> {
        self.archive
            .append_batch(lines)
            .map_err(|error| format!("{error:?}"))
    }
}
