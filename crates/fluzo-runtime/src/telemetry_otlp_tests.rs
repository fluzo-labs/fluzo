//! Unit tests for the OTLP mapping and the export authorization gate.

use super::*;
use fluzo_core::telemetry::{
    LogLevel, LogPayload, MetricKind, MetricSnapshot, MetricTemporality, SignalKind, SpanRecord,
    SpanStatus,
};
use std::collections::BTreeMap;

fn resource() -> ResourceIdentity {
    ResourceIdentity {
        service_name: "fluzo".into(),
        service_version: "0.1.0".into(),
    }
}

fn trace() -> TraceRef {
    TraceRef {
        trace_id: "0af7651916cd43dd8448eb211c80319c".into(),
        span_id: "b7ad6b7169203331".into(),
    }
}

fn span_record(sampled: bool) -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Span,
        1_700_000_001_000,
        resource(),
        Some(trace()),
        DiagnosticPayload::Span(SpanRecord {
            name: "chat flash-halogen".into(),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_500,
            status: SpanStatus::Ok,
            attributes: BTreeMap::from([
                ("gen_ai.operation.name".into(), "chat".into()),
                ("gen_ai.request.model".into(), "flash-halogen".into()),
            ]),
            sampled,
        }),
    )
}

fn counter_record() -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_001_000,
        resource(),
        None,
        DiagnosticPayload::Metric(MetricSnapshot {
            name: fluzo_core::telemetry::instruments::USAGE_INPUT_TOKENS.into(),
            unit: fluzo_core::telemetry::TOKEN_UNIT.into(),
            kind: MetricKind::Counter,
            temporality: MetricTemporality::Delta,
            window_start_ms: 1_700_000_000_000,
            window_end_ms: 1_700_000_001_000,
            dimensions: BTreeMap::from([("gen_ai.token.modality".into(), "unknown".into())]),
            aggregation: MetricAggregation::Sum { value: 120.0 },
        }),
    )
}

fn encode_line(record: &DiagnosticRecord) -> String {
    serde_json::to_string(record).unwrap()
}

#[test]
fn export_is_disabled_by_default_and_never_builds_a_sink() {
    let settings = Settings::default();
    assert!(!settings.telemetry.export_enabled);
    assert_eq!(OtlpExportConfig::from_settings(&settings).unwrap(), None);
}

#[test]
fn enabling_export_without_a_valid_endpoint_is_an_error_not_a_noop() {
    let mut settings = Settings::default();
    settings.telemetry.export_enabled = true;
    assert_eq!(
        OtlpExportConfig::from_settings(&settings).unwrap_err(),
        OtlpError::MissingEndpoint
    );
    settings.telemetry.otlp_endpoint = Some("not-a-url".into());
    assert_eq!(
        OtlpExportConfig::from_settings(&settings).unwrap_err(),
        OtlpError::InvalidEndpoint
    );
    settings.telemetry.otlp_endpoint = Some("https://collector.example".into());
    // Only http is accepted by the endpoint validator used by discovery
    // and export alike; https endpoints need a reviewed TLS story first.
    assert_eq!(
        OtlpExportConfig::from_settings(&settings).unwrap_err(),
        OtlpError::InvalidEndpoint
    );
}

#[test]
fn explicit_endpoint_builds_bounded_config() {
    let mut settings = Settings::default();
    settings.telemetry.export_enabled = true;
    settings.telemetry.otlp_endpoint = Some("http://127.0.0.1:4318".into());
    let config = OtlpExportConfig::from_settings(&settings)
        .unwrap()
        .expect("explicitly enabled export builds a config");
    assert_eq!(config.endpoint, "127.0.0.1:4318");
    assert_eq!(config.timeout, Duration::from_secs(10));
    assert_eq!(config.batch_records, 64);
}

#[test]
fn span_mapping_pins_kind_ids_times_and_status() {
    let line = encode_line(&span_record(true));
    let requests = OtlpHttpSink::build_requests(&[line]).unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "/v1/traces");
    let decoded =
        opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(
            requests[0].1.as_slice(),
        )
        .unwrap();
    let span = &decoded.resource_spans[0].scope_spans[0].spans[0];
    assert_eq!(span.name, "chat flash-halogen");
    assert_eq!(span.kind, proto::span::SpanKind::Client as i32);
    assert_eq!(
        span.trace_id,
        hex_bytes("0af7651916cd43dd8448eb211c80319c", 16).unwrap()
    );
    assert_eq!(span.span_id, hex_bytes("b7ad6b7169203331", 8).unwrap());
    assert_eq!(span.start_time_unix_nano, 1_700_000_000_000_000_000);
    assert_eq!(span.end_time_unix_nano, 1_700_000_001_500_000_000);
    assert_eq!(
        span.status.as_ref().unwrap().code,
        proto::StatusCode::Ok as i32
    );
    let resource = decoded.resource_spans[0].resource.as_ref().unwrap();
    let keys: Vec<&str> = resource
        .attributes
        .iter()
        .map(|kv| kv.key.as_str())
        .collect();
    assert_eq!(keys, ["service.name", "service.version"]);
}

#[test]
fn unsampled_span_is_not_exported_but_metrics_are_independent() {
    let unsampled = encode_line(&span_record(false));
    let metric = encode_line(&counter_record());
    let requests = OtlpHttpSink::build_requests(&[unsampled, metric]).unwrap();
    // No /v1/traces request at all: sampling excluded the only span.
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "/v1/metrics");
    let decoded =
        opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(
            requests[0].1.as_slice(),
        )
        .unwrap();
    let metric = &decoded.resource_metrics[0].scope_metrics[0].metrics[0];
    assert_eq!(
        metric.name,
        fluzo_core::telemetry::instruments::USAGE_INPUT_TOKENS
    );
    assert_eq!(metric.unit, "{token}");
    match metric.data.as_ref().unwrap() {
        proto::metric::Data::Sum(sum) => {
            assert_eq!(
                sum.aggregation_temporality,
                proto::AggregationTemporality::Delta as i32
            );
            assert!(sum.is_monotonic);
            assert_eq!(sum.data_points.len(), 1);
            match sum.data_points[0].value.unwrap() {
                proto::number_data_point::Value::AsDouble(v) => assert_eq!(v, 120.0),
                _ => panic!("expected double value"),
            }
        }
        _ => panic!("expected sum data"),
    }
}

#[test]
fn log_mapping_pins_severity_body_and_trace_correlation() {
    let record = DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_002_000,
        resource(),
        Some(trace()),
        DiagnosticPayload::Log(LogPayload {
            level: LogLevel::Warn,
            message: "collector slow".into(),
            attributes: BTreeMap::from([("fluzo.inference.purpose".into(), "agent_turn".into())]),
        }),
    );
    let requests = OtlpHttpSink::build_requests(&[encode_line(&record)]).unwrap();
    assert_eq!(requests[0].0, "/v1/logs");
    let decoded =
        opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(
            requests[0].1.as_slice(),
        )
        .unwrap();
    let log = &decoded.resource_logs[0].scope_logs[0].log_records[0];
    assert_eq!(log.severity_number, proto::SeverityNumber::Warn as i32);
    assert_eq!(log.severity_text, "WARN");
    match log.body.as_ref().unwrap().value.as_ref().unwrap() {
        proto::any_value::Value::StringValue(message) => {
            assert_eq!(message, "collector slow")
        }
        _ => panic!("expected string body"),
    }
    assert_eq!(
        log.trace_id,
        hex_bytes("0af7651916cd43dd8448eb211c80319c", 16).unwrap()
    );
}

#[test]
fn counter_with_histogram_aggregation_fails_visibly() {
    let record = DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_001_000,
        resource(),
        None,
        DiagnosticPayload::Metric(MetricSnapshot {
            name: "bad.instrument".into(),
            unit: "1".into(),
            kind: MetricKind::Counter,
            temporality: MetricTemporality::Delta,
            window_start_ms: 0,
            window_end_ms: 1,
            dimensions: BTreeMap::new(),
            aggregation: MetricAggregation::Histogram {
                count: 1,
                sum: 1.0,
                bounds: vec![],
                bucket_counts: vec![1],
            },
        }),
    );
    assert_eq!(
        OtlpHttpSink::build_requests(&[encode_line(&record)]).unwrap_err(),
        OtlpError::MetricKindMismatch
    );
}

#[test]
fn malformed_line_is_refused_as_invalid_record() {
    assert_eq!(
        OtlpHttpSink::build_requests(&["{ not json".into()]).unwrap_err(),
        OtlpError::InvalidRecord
    );
}

#[test]
fn endpoint_parser_rejects_ambiguous_authorities() {
    for bad in [
        "http://",
        "http://host:0",
        "http://host/path",
        "http://user@host",
        "ftp://host:4318",
        "http://host?q=1",
    ] {
        assert!(Target::parse(bad).is_none(), "must reject {bad}");
    }
    assert!(Target::parse("http://127.0.0.1:4318").is_some());
    assert!(Target::parse("http://collector.local:4318/v1").is_some());
}
