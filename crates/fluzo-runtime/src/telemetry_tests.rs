//! OBS-01 deterministic tests: audit separation, bounded fan-out,
//! redaction-before-persistence, drop semantics, failed-sink
//! responsiveness and shutdown deadlines. No terminal, no network.

use crate::store::{StateUpdate, Store, StoreError};
use crate::telemetry::*;
use crate::telemetry_archive::{ArchiveLine, JsonlArchive, read_archive};
use crate::telemetry_audit::AuditTrail;
use fluzo_core::telemetry::{
    AuditCategory, AuditRecord, DiagnosticPayload, DiagnosticRecord, LogPayload, MetricAggregation,
    MetricKind, MetricSnapshot, MetricTemporality, ResourceIdentity, SignalKind, SpanKind,
    SpanRecord, SpanStatus, TOKEN_UNIT,
};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fluzo-obs-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn log_record(message: &str, attributes: &[(&str, &str)]) -> DiagnosticRecord {
    let mut map = std::collections::BTreeMap::new();
    for (key, value) in attributes {
        map.insert((*key).to_string(), (*value).to_string());
    }
    DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Log(LogPayload {
            level: LogLevel::Info,
            message: message.into(),
            attributes: map,
        }),
    )
}

use fluzo_core::telemetry::LogLevel;

/// Sink that records everything it receives in memory. Each instance is
/// named so per-sink health can be attributed: the hub keeps independent
/// state per sink and several test sinks share this type.
#[derive(Clone, Default)]
struct MemorySink {
    sink_name: String,
    lines: Arc<Mutex<Vec<String>>>,
    gate: Option<Arc<Mutex<()>>>,
    fail: bool,
}

impl MemorySink {
    fn new(name: &str) -> Self {
        Self {
            sink_name: name.into(),
            ..Default::default()
        }
    }

    /// A sink whose write blocks on `gate`, simulating a stuck sink.
    fn blocked(name: &str, gate: Arc<Mutex<()>>) -> Self {
        Self {
            sink_name: name.into(),
            gate: Some(gate),
            ..Default::default()
        }
    }

    fn failing(name: &str) -> Self {
        Self {
            sink_name: name.into(),
            fail: true,
            ..Default::default()
        }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl DiagnosticSink for MemorySink {
    fn name(&self) -> String {
        self.sink_name.clone()
    }
    fn write_batch(&mut self, lines: &[String]) -> Result<(), String> {
        if let Some(gate) = &self.gate {
            let _guard = gate.lock().unwrap();
        }
        if self.fail {
            return Err("simulated sink failure".into());
        }
        self.lines.lock().unwrap().extend(lines.iter().cloned());
        Ok(())
    }
}

fn hub_with(sinks: Vec<MemorySink>, limits: HubLimits, policy: RedactionPolicy) -> DiagnosticHub {
    let mut hub = DiagnosticHub::new(limits, policy);
    for sink in sinks {
        hub.add_sink(Box::new(sink));
    }
    hub
}

fn wait_until_idle(sinks: &[MemorySink], expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while sinks
        .iter()
        .map(|s| s.lines.lock().unwrap().len())
        .sum::<usize>()
        < expected
    {
        assert!(Instant::now() < deadline, "sinks did not drain");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn publish_fans_out_to_every_sink_with_versioned_envelope() {
    let sink_a = MemorySink::new("a");
    let sink_b = MemorySink::new("b");
    let mut hub = hub_with(
        vec![sink_a.clone(), sink_b.clone()],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    let outcome = hub.publish(log_record("hello", &[("component", "test")]));
    assert_eq!(outcome.delivered, 2);
    assert_eq!(outcome.dropped, 0);
    wait_until_idle(&[sink_a.clone(), sink_b.clone()], 2);
    let lines = sink_a.lines.lock().unwrap();
    let value: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(value["format_version"], 1);
    assert_eq!(value["signal"], "Log");
    assert_eq!(value["resource"]["service_name"], "fluzo-test");
    assert_eq!(value["payload"]["Log"]["message"], "hello");
    let _ = hub.shutdown(Duration::from_secs(5));
}

#[test]
fn redaction_happens_before_any_persistence() {
    let root = Root::new();
    let archive = JsonlArchive::open(&root.0).unwrap();
    let mut hub = DiagnosticHub::new(
        HubLimits::default(),
        RedactionPolicy {
            forbidden_key_substrings: vec!["authorization".into(), "api_key".into()],
            known_safe_keys: Vec::new(),
            secret_values: vec!["sk-leaked-123".into()],
        },
    );
    hub.add_sink(Box::new(JsonlArchiveSink::new(archive)));
    hub.publish(log_record(
        "request with sk-leaked-123 inside",
        &[
            ("authorization", "Bearer super-secret"),
            ("api_key_header", "abc"),
            ("component", "gateway"),
        ],
    ));
    let report = hub.shutdown(Duration::from_secs(5));
    assert!(report.incomplete_flushes.is_empty());
    // Read the file back raw: the secret must not exist on disk anywhere.
    let raw = fs::read_to_string(JsonlArchive::segment_path(
        &root.0.join(".fluzodrive").join("telemetry"),
        0,
    ))
    .unwrap();
    assert!(!raw.contains("sk-leaked-123"));
    assert!(!raw.contains("super-secret"));
    assert!(raw.contains("[REDACTED]"));
    assert!(raw.contains("gateway")); // non-secret attributes survive
}

#[test]
fn oversized_record_is_refused_not_queued() {
    let sink = MemorySink::default();
    let limits = HubLimits {
        max_record_bytes: 256,
        ..HubLimits::default()
    };
    let mut hub = hub_with(vec![sink.clone()], limits, RedactionPolicy::default());
    let big = "x".repeat(4096);
    let outcome = hub.publish(log_record(&big, &[]));
    assert_eq!(outcome.delivered, 0);
    assert_eq!(outcome.drop_reason.as_deref(), Some("record_too_large"));
    let _ = hub.shutdown(Duration::from_secs(5));
    assert!(sink.lines.lock().unwrap().is_empty());
}

#[test]
fn queue_full_drops_without_blocking_the_producer() {
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let limits = HubLimits {
        queue_records: 2,
        ..HubLimits::default()
    };
    let blocked = MemorySink::blocked("blocked", Arc::clone(&gate));
    let healthy = MemorySink::new("healthy");
    let mut hub = hub_with(
        vec![blocked, healthy.clone()],
        limits,
        RedactionPolicy::default(),
    );
    let published = 5usize;
    let start = Instant::now();
    let mut refused = 0usize;
    for i in 0..published {
        let outcome = hub.publish(log_record(&format!("m{i}"), &[]));
        refused += outcome.dropped;
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "publish blocked"
        );
    }
    assert!(
        refused > 0,
        "a held sink must cause refusals, got {refused}"
    );

    let health = hub.health();
    let blocked_health = health.iter().find(|h| h.name == "blocked").unwrap();
    assert!(
        blocked_health.dropped.contains_key("queue_full"),
        "refusals must be attributed to the bounded queue"
    );
    assert!(
        !blocked_health.dropped.contains_key("write_failed"),
        "a stuck-but-healthy sink is not a write failure"
    );

    // The healthy sink shares the same small bound, so a fast producer
    // may legitimately refuse into it too. The contract is accounting,
    // not delivery of everything published: every record the healthy
    // sink accepted arrives exactly once and none is lost.
    let healthy_health = health.iter().find(|h| h.name == "healthy").unwrap();
    let healthy_dropped = healthy_health
        .dropped
        .get("queue_full")
        .copied()
        .unwrap_or(0) as usize;
    let accepted = published - healthy_dropped;
    assert!(accepted > 0, "healthy sink accepted nothing");
    assert!(!healthy_health.degraded);
    let deadline = Instant::now() + Duration::from_secs(5);
    while healthy.lines().len() < accepted {
        assert!(
            Instant::now() < deadline,
            "healthy sink lost accepted records"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let delivered = healthy.lines();
    assert_eq!(delivered.len(), accepted, "no accepted record may be lost");
    assert_eq!(
        delivered
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        accepted,
        "no duplicate delivery"
    );
    drop(held);
    let _ = hub.shutdown(Duration::from_secs(5));
}

#[test]
fn failing_sink_marks_degraded_without_recursive_storm() {
    let failing = MemorySink::failing("failing");
    let healthy = MemorySink::new("healthy");
    let mut hub = hub_with(
        vec![failing, healthy.clone()],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    for i in 0..3 {
        hub.publish(log_record(&format!("m{i}"), &[]));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let health = hub.health();
        let failing_health = health.iter().filter(|h| h.degraded).count();
        if failing_health == 1 && healthy.lines.lock().unwrap().len() == 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failing sink never marked degraded"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = hub.shutdown(Duration::from_secs(5));
    // Exactly three records were produced in total: the failing sink's
    // errors created no extra diagnostics through the same pipeline.
    assert_eq!(healthy.lines.lock().unwrap().len(), 3);
    let health = hub.health();
    let failed = health.iter().find(|h| h.degraded).unwrap();
    assert_eq!(*failed.dropped.get("write_failed").unwrap(), 3);
    assert!(failed.last_error.is_some());
}

#[test]
fn shutdown_drains_healthy_sinks_and_stops_admission() {
    let sink = MemorySink::new("sink");
    let mut hub = hub_with(
        vec![sink.clone()],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    hub.publish(log_record("before-shutdown", &[]));
    let report = hub.shutdown(Duration::from_secs(5));
    assert_eq!(report.drained_sinks.len(), 1);
    assert!(report.incomplete_flushes.is_empty());
    assert_eq!(sink.lines.lock().unwrap().len(), 1);
}

#[test]
fn publish_after_shutdown_is_refused_as_stopped() {
    let sink = MemorySink::new("sink");
    let mut hub = hub_with(
        vec![sink.clone()],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    let _ = hub.shutdown(Duration::from_secs(5));
    let outcome = hub.publish(log_record("too-late", &[]));
    assert_eq!(outcome.delivered, 0);
    assert_eq!(outcome.drop_reason.as_deref(), Some("shutdown_stopped"));
    assert!(sink.lines.lock().unwrap().is_empty());
}

#[test]
fn shutdown_reports_incomplete_flush_when_deadline_expires() {
    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let blocked = MemorySink::blocked("blocked", Arc::clone(&gate));
    let mut hub = hub_with(
        vec![blocked],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    for i in 0..4 {
        hub.publish(log_record(&format!("m{i}"), &[]));
    }
    let report = hub.shutdown(Duration::from_millis(50));
    assert!(
        !report.incomplete_flushes.is_empty(),
        "blocked sink must report an incomplete flush"
    );
    drop(held); // let the detached worker finish
}

#[test]
fn jsonl_archive_rotates_and_reader_flags_incomplete_trailing_record() {
    let root = Root::new();
    let mut archive = JsonlArchive::open(&root.0).unwrap().with_rotation(200);
    for i in 0..6 {
        archive
            .append_batch(&[format!("{{\"n\":{i},\"pad\":\"{}\"}}", "p".repeat(60))])
            .unwrap();
    }
    let dir = root.0.join(".fluzodrive").join("telemetry");
    let segments: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("segment-"))
        .collect();
    assert!(segments.len() > 1, "rotation must create multiple segments");
    // Simulate a crash mid-write: append a partial trailing record.
    let last = JsonlArchive::segment_path(&dir, 2);
    if !last.exists() {
        // rotate may have skipped index 2; use the newest existing segment.
        let newest = JsonlArchive::segment_path(&dir, 1);
        fs::OpenOptions::new()
            .append(true)
            .open(&newest)
            .unwrap()
            .write_all(b"{\"partial\":tru")
            .unwrap();
    } else {
        fs::OpenOptions::new()
            .append(true)
            .open(&last)
            .unwrap()
            .write_all(b"{\"partial\":tru")
            .unwrap();
    }
    let scan = read_archive(&dir).unwrap();
    let incomplete: Vec<_> = scan
        .lines
        .iter()
        .filter(|l| matches!(l, ArchiveLine::IncompleteTrailing { .. }))
        .collect();
    assert_eq!(
        incomplete.len(),
        1,
        "trailing partial record must be reported"
    );
    // Prior complete records are all still present.
    let complete = scan
        .lines
        .iter()
        .filter(|l| matches!(l, ArchiveLine::Complete(_)))
        .count();
    assert_eq!(complete, 6);
}

fn audit_record(id: &str) -> AuditRecord {
    AuditRecord {
        id: id.into(),
        category: AuditCategory::PermissionDecision,
        subject: "task:demo".into(),
        action: "tool:file_edit".into(),
        outcome: "allowed".into(),
        correlation_id: "corr-1".into(),
        detail: "fixture".into(),
        created_unix_ms: 1_700_000_000_000,
    }
}

#[test]
fn audit_and_dependent_updates_commit_together() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let update = StateUpdate {
        id: "task-1".into(),
        kind: "task".into(),
        payload: "authorized".into(),
    };
    rt.block_on(AuditTrail::new(&mut store).commit_with(
        &[audit_record("a1")],
        std::slice::from_ref(&update),
        &[],
    ))
    .unwrap();
    assert_eq!(
        rt.block_on(AuditTrail::new(&mut store).audit_count())
            .unwrap(),
        1
    );
    assert!(rt.block_on(store.state("task-1")).unwrap().is_some());
}

#[test]
fn audit_commit_gate_blocks_dependent_effect_on_audit_failure() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    rt.block_on(AuditTrail::new(&mut store).commit(&[audit_record("dup")]))
        .unwrap();
    // Duplicate audit id: the mandatory record cannot commit, so the
    // dependent state update in the same unit must not land either.
    let update = StateUpdate {
        id: "effect-1".into(),
        kind: "task".into(),
        payload: "must-not-appear".into(),
    };
    let result = rt.block_on(AuditTrail::new(&mut store).commit_with(
        &[audit_record("dup")],
        &[update],
        &[],
    ));
    assert_eq!(result.unwrap_err(), StoreError::Constraint);
    assert!(rt.block_on(store.state("effect-1")).unwrap().is_none());
    assert_eq!(
        rt.block_on(AuditTrail::new(&mut store).audit_count())
            .unwrap(),
        1
    );
}

#[test]
fn diagnostics_never_substitute_for_audit_records() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let archive = JsonlArchive::open(&root.0).unwrap();
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(Box::new(JsonlArchiveSink::new(archive)));
    for i in 0..5 {
        hub.publish(log_record(&format!("diagnostic {i}"), &[]));
    }
    let _ = hub.shutdown(Duration::from_secs(5));
    // Diagnostics landed locally but produced zero audit rows: the
    // required audit path is separate and cannot be satisfied by the
    // diagnostic sinks.
    assert_eq!(
        rt.block_on(AuditTrail::new(&mut store).audit_count())
            .unwrap(),
        0
    );
    let dir = root.0.join(".fluzodrive").join("telemetry");
    let scan = read_archive(&dir).unwrap();
    assert_eq!(
        scan.lines
            .iter()
            .filter(|l| matches!(l, ArchiveLine::Complete(_)))
            .count(),
        5
    );
}

#[test]
fn audit_failure_does_not_disable_diagnostic_sinks() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    rt.block_on(AuditTrail::new(&mut store).commit(&[audit_record("x")]))
        .unwrap();
    // Mandatory audit fails (duplicate id)...
    assert!(
        rt.block_on(AuditTrail::new(&mut store).commit(&[audit_record("x")]))
            .is_err()
    );
    // ...while diagnostics keep working independently.
    let sink = MemorySink::new("sink");
    let mut hub = hub_with(
        vec![sink.clone()],
        HubLimits::default(),
        RedactionPolicy::default(),
    );
    let outcome = hub.publish(log_record("still flowing", &[]));
    assert_eq!(outcome.delivered, 1);
    wait_until_idle(std::slice::from_ref(&sink), 1);
    let _ = hub.shutdown(Duration::from_secs(5));
    assert_eq!(sink.lines.lock().unwrap().len(), 1);
}

#[test]
fn archive_root_symlink_is_refused() {
    let root = Root::new();
    let outside = Root::new();
    fs::create_dir_all(root.0.join(".fluzodrive")).unwrap();
    std::os::unix::fs::symlink(&outside.0, root.0.join(".fluzodrive").join("telemetry")).unwrap();
    let result = JsonlArchive::open(&root.0);
    assert!(result.is_err(), "symlinked archive root fails closed");
}

fn metric_record(temporality: MetricTemporality) -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_001_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Metric(MetricSnapshot {
            name: fluzo_core::telemetry::instruments::USAGE_INPUT_TOKENS.to_string(),
            unit: TOKEN_UNIT.to_string(),
            kind: MetricKind::Counter,
            temporality,
            window_start_ms: 1_700_000_000_000,
            window_end_ms: 1_700_000_001_000,
            dimensions: std::collections::BTreeMap::from([(
                fluzo_core::telemetry::attributes::TOKEN_MODALITY.to_string(),
                "text".to_string(),
            )]),
            aggregation: MetricAggregation::Sum { value: 120.0 },
        }),
    )
}

#[test]
fn metric_envelope_round_trips_through_the_archive_with_temporality() {
    // The archive stores the documented envelope, not an SDK-side copy:
    // instrument type, unit, aggregation window and temporality all
    // survive serialization and can be read back off disk.
    let root = Root::new();
    let archive = JsonlArchive::open(&root.0).unwrap();
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(Box::new(JsonlArchiveSink::new(archive)));
    hub.publish(metric_record(MetricTemporality::Delta));
    hub.publish(metric_record(MetricTemporality::Cumulative));
    let _ = hub.shutdown(Duration::from_secs(5));

    let dir = root.0.join(".fluzodrive").join("telemetry");
    let scan = read_archive(&dir).unwrap();
    let mut seen = Vec::new();
    for line in scan.lines.iter() {
        let ArchiveLine::Complete(text) = line else {
            continue;
        };
        let record: DiagnosticRecord = serde_json::from_str(text).unwrap();
        assert_eq!(record.format_version, 1);
        assert_eq!(record.signal, SignalKind::Metric);
        if let DiagnosticPayload::Metric(snapshot) = record.payload {
            assert_eq!(snapshot.unit, "{token}");
            assert_eq!(snapshot.kind, MetricKind::Counter);
            assert_eq!(
                snapshot
                    .dimensions
                    .get(fluzo_core::telemetry::attributes::TOKEN_MODALITY)
                    .map(String::as_str),
                Some("text")
            );
            seen.push(snapshot.temporality);
        }
    }
    assert_eq!(
        seen,
        vec![MetricTemporality::Delta, MetricTemporality::Cumulative]
    );
}

#[test]
fn redaction_covers_span_attributes_and_metric_dimensions_not_just_logs() {
    // Each payload arm carries its own attribute map. A policy that only
    // scrubbed log attributes would leak a credential through a span or
    // a metric dimension, so all three arms are checked against the file
    // actually written.
    let root = Root::new();
    let archive = JsonlArchive::open(&root.0).unwrap();
    let mut hub = DiagnosticHub::new(
        HubLimits::default(),
        RedactionPolicy {
            forbidden_key_substrings: vec![
                "authorization".into(),
                "api_key".into(),
                "token".into(),
            ],
            // The pinned GenAI exemption is what keeps `token.modality`
            // readable while the broad `token` rule still catches an
            // arbitrary credential-shaped key.
            known_safe_keys: vec!["gen_ai.token.modality".into()],
            secret_values: vec!["sk-leaked-123".into()],
        },
    );
    hub.add_sink(Box::new(JsonlArchiveSink::new(archive)));

    hub.publish(DiagnosticRecord::new(
        SignalKind::Span,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Span(SpanRecord {
            name: fluzo_core::telemetry::gen_ai_span_name("chat", "flash-halogen"),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_000,
            status: SpanStatus::Ok,
            attributes: std::collections::BTreeMap::from([
                (
                    "authorization".to_string(),
                    "Bearer sk-leaked-123".to_string(),
                ),
                (
                    fluzo_core::telemetry::attributes::REQUEST_MODEL.to_string(),
                    "flash-halogen".to_string(),
                ),
            ]),
            sampled: true,
        }),
    ));

    let mut dimensions = std::collections::BTreeMap::new();
    dimensions.insert("api_key_label".to_string(), "sk-leaked-123".to_string());
    dimensions.insert(
        fluzo_core::telemetry::attributes::TOKEN_MODALITY.to_string(),
        "text".to_string(),
    );
    hub.publish(DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_001_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Metric(MetricSnapshot {
            name: fluzo_core::telemetry::instruments::OPERATION_DURATION.to_string(),
            unit: "s".to_string(),
            kind: MetricKind::Histogram,
            temporality: MetricTemporality::Delta,
            window_start_ms: 1_700_000_000_000,
            window_end_ms: 1_700_000_002_000,
            dimensions,
            aggregation: MetricAggregation::Histogram {
                count: 1,
                sum: 1.5,
                bounds: vec![1.0, 2.0],
                bucket_counts: vec![1, 0],
            },
        }),
    ));

    let _ = hub.shutdown(Duration::from_secs(5));
    let raw = fs::read_to_string(JsonlArchive::segment_path(
        &root.0.join(".fluzodrive").join("telemetry"),
        0,
    ))
    .unwrap();
    assert!(!raw.contains("sk-leaked-123"), "secret leaked to disk");
    assert!(!raw.contains("Bearer"), "credential leaked to disk");
    assert!(raw.contains("[REDACTED]"));
    // Non-secret semantic attributes survive.
    assert!(raw.contains("flash-halogen"));
    assert!(raw.contains("text"));
}

#[test]
fn default_policy_preserves_pinned_genai_usage_attributes() {
    // The broad `token` substring rule catches credential-shaped keys,
    // but the pinned GenAI vocabulary legitimately contains "token".
    // Without the exact-key exemption the default policy silently erases
    // the token accounting the contract requires, so this pins both sides.
    let policy = RedactionPolicy::default();
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert(
        fluzo_core::telemetry::attributes::USAGE_INPUT_TOKENS.to_string(),
        "100".to_string(),
    );
    attributes.insert(
        fluzo_core::telemetry::attributes::USAGE_OUTPUT_TOKENS.to_string(),
        "20".to_string(),
    );
    attributes.insert(
        fluzo_core::telemetry::attributes::USAGE_CACHE_READ_INPUT_TOKENS.to_string(),
        "40".to_string(),
    );
    attributes.insert(
        fluzo_core::telemetry::attributes::USAGE_REASONING_OUTPUT_TOKENS.to_string(),
        "5".to_string(),
    );
    attributes.insert(
        fluzo_core::telemetry::attributes::TOKEN_MODALITY.to_string(),
        "text".to_string(),
    );
    let mut record = DiagnosticRecord::new(
        SignalKind::Span,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Span(SpanRecord {
            name: fluzo_core::telemetry::gen_ai_span_name("chat", "flash-halogen"),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_000,
            status: SpanStatus::Ok,
            attributes,
            sampled: true,
        }),
    );
    redact_record(&mut record, &policy);
    let DiagnosticPayload::Span(span) = &record.payload else {
        panic!("expected span");
    };
    for key in [
        fluzo_core::telemetry::attributes::USAGE_INPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_OUTPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_CACHE_READ_INPUT_TOKENS,
        fluzo_core::telemetry::attributes::USAGE_REASONING_OUTPUT_TOKENS,
        fluzo_core::telemetry::attributes::TOKEN_MODALITY,
    ] {
        assert_eq!(
            span.attributes.get(key).map(String::as_str),
            match key {
                fluzo_core::telemetry::attributes::USAGE_INPUT_TOKENS => Some("100"),
                fluzo_core::telemetry::attributes::USAGE_OUTPUT_TOKENS => Some("20"),
                fluzo_core::telemetry::attributes::USAGE_CACHE_READ_INPUT_TOKENS => Some("40"),
                fluzo_core::telemetry::attributes::USAGE_REASONING_OUTPUT_TOKENS => Some("5"),
                _ => Some("text"),
            },
            "pinned GenAI attribute {key} must survive the default policy"
        );
    }
    // The broad rule still catches an arbitrary credential-shaped key.
    let mut leaky = std::collections::BTreeMap::new();
    leaky.insert("access_token".to_string(), "abc123".to_string());
    leaky.insert("refresh token".to_string(), "def456".to_string());
    let mut leak_record = DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Log(LogPayload {
            level: LogLevel::Info,
            message: "x".into(),
            attributes: leaky,
        }),
    );
    redact_record(&mut leak_record, &policy);
    let DiagnosticPayload::Log(log) = &leak_record.payload else {
        panic!("expected log");
    };
    assert_eq!(log.attributes["access_token"], "[REDACTED]");
    assert_eq!(log.attributes["refresh token"], "[REDACTED]");
}

#[test]
fn every_pinned_vocabulary_key_survives_default_policy() {
    // T20 sweep: the default policy's broad credential substrings must
    // never rewrite a legitimate pinned vocabulary key. The table is
    // built from the core constants, so a future pinned key that
    // introduces a forbidden substring without a matching exemption in
    // pinned_safe_genai_keys() fails here rather than silently erasing
    // structured data in production.
    use fluzo_core::telemetry::attributes as a;
    let pinned: &[&str] = &[
        a::OPERATION_NAME,
        a::PROVIDER_NAME,
        a::REQUEST_MODEL,
        a::RESPONSE_MODEL,
        a::REQUEST_STREAM,
        a::REQUEST_MAX_TOKENS,
        a::RESPONSE_ID,
        a::RESPONSE_FINISH_REASONS,
        a::USAGE_INPUT_TOKENS,
        a::USAGE_OUTPUT_TOKENS,
        a::USAGE_CACHE_READ_INPUT_TOKENS,
        a::USAGE_CACHE_WRITE_INPUT_TOKENS,
        a::USAGE_REASONING_OUTPUT_TOKENS,
        a::RESPONSE_TIME_TO_FIRST_CHUNK,
        a::TOKEN_MODALITY,
        a::CONVERSATION_ID,
        a::ERROR_TYPE,
        a::FLUZO_INFERENCE_PURPOSE,
        a::ABSENT_BY_DEFAULT[0],
        a::ABSENT_BY_DEFAULT[1],
        a::ABSENT_BY_DEFAULT[2],
    ];

    let mut attributes = std::collections::BTreeMap::new();
    for key in pinned {
        // A distinctive legitimate value per key; if the key were
        // redacted the value would become [REDACTED] and the assert
        // below catches it.
        attributes.insert((*key).to_string(), format!("value-for-{key}"));
    }

    let policy = RedactionPolicy::default();
    let mut record = DiagnosticRecord::new(
        SignalKind::Span,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Span(SpanRecord {
            name: fluzo_core::telemetry::gen_ai_span_name("chat", "flash-halogen"),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_000,
            status: SpanStatus::Ok,
            attributes,
            sampled: true,
        }),
    );
    redact_record(&mut record, &policy);
    let DiagnosticPayload::Span(span) = &record.payload else {
        panic!("expected span");
    };
    for key in pinned {
        assert_eq!(
            span.attributes.get(*key).map(String::as_str),
            Some(format!("value-for-{key}").as_str()),
            "pinned vocabulary key {key} must survive the default policy unchanged"
        );
    }
}

#[test]
fn pinned_instrument_names_are_never_key_redacted() {
    // Instrument names legitimately contain "token" but are metric
    // NAMES (String fields), never map keys, so the key-substring rule
    // must not touch them. A metric whose name contains "token" keeps
    // its name; only credential-shaped dimension keys are redacted.
    use fluzo_core::telemetry::instruments as i;
    let policy = RedactionPolicy::default();
    let mut dimensions = std::collections::BTreeMap::new();
    dimensions.insert(
        fluzo_core::telemetry::attributes::TOKEN_MODALITY.to_string(),
        "text".to_string(),
    );
    dimensions.insert("api_key".to_string(), "sk-secret".to_string());
    let mut record = DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Metric(MetricSnapshot {
            name: i::USAGE_INPUT_TOKENS.to_string(),
            unit: fluzo_core::telemetry::TOKEN_UNIT.to_string(),
            kind: MetricKind::Counter,
            temporality: MetricTemporality::Delta,
            window_start_ms: 1_700_000_000_000,
            window_end_ms: 1_700_000_001_000,
            dimensions,
            aggregation: MetricAggregation::Sum { value: 0.0 },
        }),
    );
    redact_record(&mut record, &policy);
    let DiagnosticPayload::Metric(metric) = &record.payload else {
        panic!("expected metric");
    };
    assert_eq!(
        metric.name,
        i::USAGE_INPUT_TOKENS,
        "instrument name containing 'token' must not be redacted"
    );
    assert_eq!(
        metric.dimensions[fluzo_core::telemetry::attributes::TOKEN_MODALITY],
        "text",
        "pinned dimension survives"
    );
    assert_eq!(
        metric.dimensions["api_key"], "[REDACTED]",
        "credential-shaped dimension still redacted"
    );
}

#[test]
fn secret_shaped_values_still_redacted_across_all_forbidden_substrings() {
    // The other side of the sweep: every forbidden substring must still
    // catch a credential-shaped key with a secret value under the
    // default policy, so widening the safe-key exemption for
    // max_tokens did not weaken the rule.
    let policy = RedactionPolicy::default();
    let leaky: &[(&str, &str)] = &[
        ("authorization", "Bearer abc"),
        ("access_token", "abc"),
        ("client_secret", "abc"),
        ("api_key", "abc"),
        ("apikey", "abc"),
        ("password", "abc"),
        ("set_cookie", "abc"),
    ];
    let mut attributes = std::collections::BTreeMap::new();
    for (key, value) in leaky {
        attributes.insert((*key).to_string(), (*value).to_string());
    }
    let mut record = DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Log(LogPayload {
            level: LogLevel::Info,
            message: "x".into(),
            attributes,
        }),
    );
    redact_record(&mut record, &policy);
    let DiagnosticPayload::Log(log) = &record.payload else {
        panic!("expected log");
    };
    for (key, _) in leaky {
        assert_eq!(
            log.attributes[*key], "[REDACTED]",
            "credential-shaped key {key} must be redacted by the default policy"
        );
    }
}
