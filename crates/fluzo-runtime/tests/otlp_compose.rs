//! OBS-02 synthetic run against the local Compose collector.
//!
//! Reuses the #105 wire contract (hub + OtlpHttpSink) with the real
//! OpenTelemetry Collector standing in for the test receiver. Ignored
//! by default: it requires the stack under `observability/` and is
//! driven by `scripts/observability_smoke.py`, never by the offline
//! suite or CI.
//!
//! Modes (FLUZO_COMPOSE_MODE):
//! - `normal`: one span, one correlated structured log and one
//!   metric sharing a trace id; asserts the export sink drained.
//! - `outage`: many publishes against a stopped collector; asserts
//!   the producer stays non-blocking and gaps are counted (same
//!   semantics as the #105 outage tests; no lossless claims).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, LogLevel, LogPayload, MetricAggregation, MetricKind,
    MetricTemporality, ResourceIdentity, SignalKind, SpanKind, SpanRecord, SpanStatus, TraceRef,
    attributes, instruments,
};
use fluzo_runtime::telemetry::{DiagnosticHub, HubLimits, RedactionPolicy};
use fluzo_runtime::telemetry_otlp::{OtlpExportConfig, OtlpHttpSink};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn hex_ids() -> (String, String) {
    let now = now_ms();
    let pid = u128::from(std::process::id());
    // Fill the full 128-bit space: some backends normalise leading
    // zeros and an all-zero high half is needlessly adversarial.
    let trace = (now << 80) ^ (pid << 48) ^ 0x9e37_79b9_7f4a_7c15_u128;
    let span = (((now << 16) ^ (pid << 40) ^ 0xdeca_fbad_0bad_c0de_u128) | 0x1) as u64;
    (format!("{trace:032x}"), format!("{span:016x}"))
}

fn resource() -> ResourceIdentity {
    ResourceIdentity {
        service_name: "fluzo".into(),
        service_version: "0.1.0".into(),
    }
}

fn build_hub(endpoint: String) -> DiagnosticHub {
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(Box::new(OtlpHttpSink::new(OtlpExportConfig {
        endpoint,
        timeout: Duration::from_secs(5),
        batch_records: 64,
    })));
    hub
}

#[test]
#[ignore = "requires the local observability compose stack; run scripts/observability_smoke.py"]
fn synthetic_run_reaches_the_compose_collector() {
    let endpoint = env_or("FLUZO_COMPOSE_OTLP_ENDPOINT", "http://127.0.0.1:4318");
    let mode = env_or("FLUZO_COMPOSE_MODE", "normal");
    let run_id = format!("smoke-{}-{}", now_ms(), std::process::id());
    let (trace_hex, span_hex) = hex_ids();
    let trace = TraceRef {
        trace_id: trace_hex.clone(),
        span_id: span_hex.clone(),
    };
    let mut hub = build_hub(endpoint);

    if mode == "outage" {
        // Collector is down: publishing must stay fast and every loss
        // must be a counted, visible gap.
        let started = Instant::now();
        for index in 0..500u32 {
            hub.publish(DiagnosticRecord::new(
                SignalKind::Metric,
                now_ms() as i64,
                resource(),
                None,
                DiagnosticPayload::Metric(fluzo_core::telemetry::MetricSnapshot {
                    name: instruments::USAGE_INPUT_TOKENS.into(),
                    unit: fluzo_core::telemetry::TOKEN_UNIT.into(),
                    kind: MetricKind::Counter,
                    temporality: MetricTemporality::Delta,
                    window_start_ms: now_ms() as i64,
                    window_end_ms: now_ms() as i64,
                    dimensions: std::collections::BTreeMap::from([(
                        attributes::TOKEN_MODALITY.into(),
                        "unknown".into(),
                    )]),
                    aggregation: MetricAggregation::Sum {
                        value: f64::from(index),
                    },
                }),
            ));
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(5),
            "outage must not block the producer: {elapsed:?}"
        );
        let health = hub.health();
        for sink in &health {
            println!(
                "FLUZO_OUTAGE_SINK name={} degraded={} queued={} dropped={:?}",
                sink.name, sink.degraded, sink.queued_records, sink.dropped
            );
        }
        let _ = hub.shutdown(Duration::from_millis(500));
        println!("FLUZO_OUTAGE_ELAPSED_MS={}", elapsed.as_millis());
        return;
    }

    // Normal mode: one correlated trio.
    let mut delivered = 0usize;
    delivered += hub
        .publish(DiagnosticRecord::new(
            SignalKind::Span,
            now_ms() as i64,
            resource(),
            Some(trace.clone()),
            DiagnosticPayload::Span(SpanRecord {
                name: fluzo_core::telemetry::gen_ai_span_name("chat", "smoke-model"),
                kind: SpanKind::Client,
                start_unix_ms: now_ms() as i64 - 250,
                end_unix_ms: now_ms() as i64,
                status: SpanStatus::Ok,
                attributes: std::collections::BTreeMap::from([
                    (attributes::OPERATION_NAME.into(), "chat".into()),
                    (attributes::REQUEST_MODEL.into(), "smoke-model".into()),
                    (attributes::USAGE_INPUT_TOKENS.into(), "120".into()),
                    ("fluzo.smoke.run_id".into(), run_id.clone()),
                ]),
                sampled: true,
            }),
        ))
        .delivered;
    delivered += hub
        .publish(DiagnosticRecord::new(
            SignalKind::Log,
            now_ms() as i64,
            resource(),
            Some(trace.clone()),
            DiagnosticPayload::Log(LogPayload {
                level: LogLevel::Info,
                // The trace id is embedded in the line so Loki users can
                // correlate without promoting run/task ids to labels.
                message: format!("synthetic smoke run {run_id} trace={trace_hex}"),
                attributes: std::collections::BTreeMap::from([(
                    "fluzo.smoke.run_id".into(),
                    run_id.clone(),
                )]),
            }),
        ))
        .delivered;
    delivered += hub
        .publish(DiagnosticRecord::new(
            SignalKind::Metric,
            now_ms() as i64,
            resource(),
            None,
            DiagnosticPayload::Metric(fluzo_core::telemetry::MetricSnapshot {
                name: instruments::USAGE_INPUT_TOKENS.into(),
                unit: fluzo_core::telemetry::TOKEN_UNIT.into(),
                kind: MetricKind::Counter,
                temporality: MetricTemporality::Delta,
                window_start_ms: now_ms() as i64 - 1_000,
                window_end_ms: now_ms() as i64,
                dimensions: std::collections::BTreeMap::from([(
                    attributes::TOKEN_MODALITY.into(),
                    "text".into(),
                )]),
                aggregation: MetricAggregation::Sum { value: 120.0 },
            }),
        ))
        .delivered;

    println!("FLUZO_SMOKE_DELIVERED={delivered}");
    assert_eq!(delivered, 3, "all three records must enter the sink queue");
    println!("FLUZO_SMOKE_TRACE_ID={trace_hex}");
    println!("FLUZO_SMOKE_SPAN_ID={span_hex}");
    println!("FLUZO_SMOKE_RUN_ID={run_id}");
    let report = hub.shutdown(Duration::from_secs(15));
    let health = hub.health();
    for sink in &health {
        println!(
            "FLUZO_SMOKE_SINK name={} degraded={} dropped={:?} last_error={:?}",
            sink.name, sink.degraded, sink.dropped, sink.last_error
        );
    }
    assert!(
        report.drained_sinks.contains(&"otlp-http".to_string()),
        "export sink must drain to the compose collector: {report:?}"
    );
    for sink in &health {
        assert!(
            !sink.degraded && sink.dropped.is_empty(),
            "export must succeed against the compose collector: {sink:?}"
        );
    }
}
