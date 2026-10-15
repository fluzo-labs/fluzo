//! OBS-01b OTLP HTTP/protobuf export sink.
//!
//! This is just another [`DiagnosticSink`] on the existing hub: it
//! receives already-redacted, already-size-bounded JSON lines, maps
//! them onto OTLP protobuf messages and POSTs them to the
//! operator-configured endpoint over the already-pinned hyper client.
//! The OpenTelemetry SDK pipeline is deliberately NOT run: the hub's
//! bounded per-sink queue stays the only queue and the hub's capture
//! boundary stays the only redaction pass. Running the SDK's batch
//! processors would add a second unredacted capture path, which the
//! OBS-01 contract forbids.
//!
//! Export is disabled by default. A sink only exists when the operator
//! explicitly enabled export AND supplied an endpoint AND the settings
//! validate; [`OtlpExportConfig::from_settings`] is that gate and no
//! socket is opened otherwise. Redirects are rejected (never followed),
//! HTTP 429 receives no automatic retry, and every request is bounded
//! by an explicit timeout. A failed batch is reported to the hub,
//! which records the gap as a counted drop; nothing is silently
//! buffered for later replay.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use fluzo_core::settings::Settings;
use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, MetricAggregation, MetricKind, MetricTemporality,
    ResourceIdentity, SpanKind, SpanStatus, TraceRef,
};
use prost::Message as _;

use crate::telemetry::DiagnosticSink;

mod proto {
    pub use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
    pub use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
    pub use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    pub use opentelemetry_proto::tonic::common::v1::{
        AnyValue, InstrumentationScope, KeyValue, any_value,
    };
    pub use opentelemetry_proto::tonic::logs::v1::{
        LogRecord, ResourceLogs, ScopeLogs, SeverityNumber,
    };
    pub use opentelemetry_proto::tonic::metrics::v1::{
        AggregationTemporality, Histogram, HistogramDataPoint, Metric, NumberDataPoint,
        ResourceMetrics, ScopeMetrics, Sum, metric, number_data_point,
    };
    pub use opentelemetry_proto::tonic::resource::v1::Resource;
    pub use opentelemetry_proto::tonic::trace::v1::{
        ResourceSpans, ScopeSpans, Span, Status, span, status::StatusCode,
    };
}

/// Hard ceiling on a collector response body we are willing to read.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Hard ceiling on the encoded size of one export request chunk.
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

/// Error categories for the export path. Categories only: an error must
/// never echo exported content back through the pipeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OtlpError {
    Disabled,
    MissingEndpoint,
    InvalidEndpoint,
    InvalidRecord,
    MetricKindMismatch,
    Connection,
    Timeout,
    /// A 3xx was received and rejected. The redirect target is never
    /// contacted, matching the all-transport redirect rule.
    RedirectRejected,
    /// HTTP 429: no automatic retry, cooldown or fallback.
    RateLimited,
    Http(u16),
}

impl OtlpError {
    fn category(&self) -> String {
        match self {
            Self::Disabled => "disabled".into(),
            Self::MissingEndpoint => "missing_endpoint".into(),
            Self::InvalidEndpoint => "invalid_endpoint".into(),
            Self::InvalidRecord => "invalid_record".into(),
            Self::MetricKindMismatch => "metric_kind_mismatch".into(),
            Self::Connection => "connection_failed".into(),
            Self::Timeout => "timeout".into(),
            Self::RedirectRejected => "redirect_rejected".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Http(code) => format!("http_{code}"),
        }
    }
}

/// Explicitly authorized export configuration. Constructed only from
/// validated settings; there is no default destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OtlpExportConfig {
    pub endpoint: String,
    pub timeout: Duration,
    pub batch_records: usize,
}

impl OtlpExportConfig {
    /// The authorization gate. Returns `Ok(None)` only when export is
    /// not enabled (the default). Enabling export without a valid
    /// endpoint is a configuration error, not a silent no-op.
    pub fn from_settings(settings: &Settings) -> Result<Option<Self>, OtlpError> {
        if !settings.telemetry.export_enabled {
            return Ok(None);
        }
        let endpoint = settings
            .telemetry
            .otlp_endpoint
            .clone()
            .ok_or(OtlpError::MissingEndpoint)?;
        let target = Target::parse(&endpoint).ok_or(OtlpError::InvalidEndpoint)?;
        if settings.telemetry.otlp_protocol != fluzo_core::settings::OtlpProtocol::HttpProtobuf {
            return Err(OtlpError::InvalidEndpoint);
        }
        if settings.telemetry.export_timeout_seconds == 0
            || settings.telemetry.export_batch_records == 0
        {
            return Err(OtlpError::InvalidEndpoint);
        }
        Ok(Some(Self {
            endpoint: target.authority,
            timeout: Duration::from_secs(settings.telemetry.export_timeout_seconds),
            batch_records: settings.telemetry.export_batch_records as usize,
        }))
    }
}

struct Target {
    host: String,
    port: u16,
    authority: String,
}

impl Target {
    fn parse(value: &str) -> Option<Self> {
        if value.len() > 2048 || !value.is_ascii() || value.bytes().any(|b| b.is_ascii_control()) {
            return None;
        }
        let uri: hyper::Uri = value.parse().ok()?;
        if uri.scheme_str() != Some("http")
            || uri.query().is_some()
            || value.contains(['#', '@', '%'])
        {
            return None;
        }
        let host = uri.host()?;
        if host.trim_matches(['[', ']']).is_empty() || uri.port_u16() == Some(0) {
            return None;
        }
        let base = uri.path().trim_end_matches('/');
        if !base.is_empty() && base != "/v1" {
            return None;
        }
        Some(Self {
            host: host.trim_matches(['[', ']']).to_owned(),
            port: uri.port_u16().unwrap_or(80),
            authority: uri.authority()?.to_string(),
        })
    }
}

fn any_string(value: String) -> proto::AnyValue {
    proto::AnyValue {
        value: Some(proto::any_value::Value::StringValue(value)),
    }
}

fn key_values(map: &BTreeMap<String, String>) -> Vec<proto::KeyValue> {
    map.iter()
        .map(|(key, value)| proto::KeyValue {
            key: key.clone(),
            value: Some(any_string(value.clone())),
            ..Default::default()
        })
        .collect()
}

fn hex_bytes(hex: &str, expected_len: usize) -> Option<Vec<u8>> {
    if hex.len() != expected_len * 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn trace_ids(trace: &Option<TraceRef>) -> Result<(Vec<u8>, Vec<u8>), OtlpError> {
    let trace = trace.as_ref().ok_or(OtlpError::InvalidRecord)?;
    Ok((
        hex_bytes(&trace.trace_id, 16).ok_or(OtlpError::InvalidRecord)?,
        hex_bytes(&trace.span_id, 8).ok_or(OtlpError::InvalidRecord)?,
    ))
}

fn proto_span(record: &DiagnosticRecord) -> Result<Option<proto::Span>, OtlpError> {
    let DiagnosticPayload::Span(span) = &record.payload else {
        return Ok(None);
    };
    // Sampling excludes span export only. Metric records are separate
    // hub records and are never gated on this flag.
    if !span.sampled {
        return Ok(None);
    }
    let (trace_id, span_id) = trace_ids(&record.trace)?;
    let (code, message) = match &span.status {
        SpanStatus::Ok => (proto::StatusCode::Ok as i32, String::new()),
        SpanStatus::Error { kind } => (proto::StatusCode::Error as i32, kind.clone()),
    };
    Ok(Some(proto::Span {
        trace_id,
        span_id,
        trace_state: String::new(),
        parent_span_id: Vec::new(),
        flags: 0,
        name: span.name.clone(),
        kind: match span.kind {
            SpanKind::Client => proto::span::SpanKind::Client as i32,
            SpanKind::Internal => proto::span::SpanKind::Internal as i32,
        },
        start_time_unix_nano: u64::try_from(span.start_unix_ms)
            .unwrap_or(0)
            .saturating_mul(1_000_000),
        end_time_unix_nano: u64::try_from(span.end_unix_ms)
            .unwrap_or(0)
            .saturating_mul(1_000_000),
        attributes: key_values(&span.attributes),
        dropped_attributes_count: 0,
        events: Vec::new(),
        dropped_events_count: 0,
        links: Vec::new(),
        dropped_links_count: 0,
        status: Some(proto::Status { message, code }),
    }))
}

fn proto_metric(
    snapshot: &fluzo_core::telemetry::MetricSnapshot,
) -> Result<proto::Metric, OtlpError> {
    let temporality = match snapshot.temporality {
        MetricTemporality::Delta => proto::AggregationTemporality::Delta as i32,
        MetricTemporality::Cumulative => proto::AggregationTemporality::Cumulative as i32,
    };
    let start = u64::try_from(snapshot.window_start_ms)
        .unwrap_or(0)
        .saturating_mul(1_000_000);
    let end = u64::try_from(snapshot.window_end_ms)
        .unwrap_or(0)
        .saturating_mul(1_000_000);
    let attributes = key_values(&snapshot.dimensions);
    let data = match (&snapshot.kind, &snapshot.aggregation) {
        (MetricKind::Counter, MetricAggregation::Sum { value }) => {
            Some(proto::metric::Data::Sum(proto::Sum {
                data_points: vec![proto::NumberDataPoint {
                    attributes,
                    start_time_unix_nano: start,
                    time_unix_nano: end,
                    exemplars: Vec::new(),
                    flags: 0,
                    value: Some(proto::number_data_point::Value::AsDouble(*value)),
                }],
                aggregation_temporality: temporality,
                is_monotonic: true,
            }))
        }
        (
            MetricKind::Histogram,
            MetricAggregation::Histogram {
                count,
                sum,
                bounds,
                bucket_counts,
            },
        ) => Some(proto::metric::Data::Histogram(proto::Histogram {
            data_points: vec![proto::HistogramDataPoint {
                attributes,
                start_time_unix_nano: start,
                time_unix_nano: end,
                count: *count,
                sum: Some(*sum),
                bucket_counts: bucket_counts.clone(),
                explicit_bounds: bounds.clone(),
                exemplars: Vec::new(),
                flags: 0,
                min: None,
                max: None,
            }],
            aggregation_temporality: temporality,
        })),
        // A counter carrying a histogram (or vice versa) is a producer
        // bug; fail visibly instead of exporting a wrong aggregation.
        _ => return Err(OtlpError::MetricKindMismatch),
    };
    Ok(proto::Metric {
        name: snapshot.name.clone(),
        description: String::new(),
        unit: snapshot.unit.clone(),
        metadata: Vec::new(),
        data,
    })
}

fn proto_log(record: &DiagnosticRecord) -> Result<Option<proto::LogRecord>, OtlpError> {
    let DiagnosticPayload::Log(log) = &record.payload else {
        return Ok(None);
    };
    let (severity_number, severity_text) = match log.level {
        fluzo_core::telemetry::LogLevel::Debug => {
            (proto::SeverityNumber::Debug as i32, "DEBUG".to_string())
        }
        fluzo_core::telemetry::LogLevel::Info => {
            (proto::SeverityNumber::Info as i32, "INFO".to_string())
        }
        fluzo_core::telemetry::LogLevel::Warn => {
            (proto::SeverityNumber::Warn as i32, "WARN".to_string())
        }
        fluzo_core::telemetry::LogLevel::Error => {
            (proto::SeverityNumber::Error as i32, "ERROR".to_string())
        }
    };
    let (trace_id, span_id) = match &record.trace {
        Some(_) => trace_ids(&record.trace)?,
        None => (Vec::new(), Vec::new()),
    };
    Ok(Some(proto::LogRecord {
        time_unix_nano: u64::try_from(record.timestamp_ms)
            .unwrap_or(0)
            .saturating_mul(1_000_000),
        observed_time_unix_nano: u64::try_from(record.timestamp_ms)
            .unwrap_or(0)
            .saturating_mul(1_000_000),
        severity_number,
        severity_text,
        body: Some(any_string(log.message.clone())),
        attributes: key_values(&log.attributes),
        dropped_attributes_count: 0,
        flags: 0,
        trace_id,
        span_id,
        event_name: String::new(),
    }))
}

/// The OTLP HTTP/protobuf export sink. One bounded hub queue feeds
/// this sink; requests are chunked to stay under fixed byte/count
/// ceilings and every request carries the configured timeout.
pub struct OtlpHttpSink {
    config: OtlpExportConfig,
}

impl OtlpHttpSink {
    pub fn new(config: OtlpExportConfig) -> Self {
        Self { config }
    }

    /// Build the protobuf request bodies for one already-redacted
    /// batch. Returns (path, body) pairs for the signals actually
    /// present; an empty vector means nothing to send.
    fn build_requests(lines: &[String]) -> Result<Vec<(&'static str, Vec<u8>)>, OtlpError> {
        let mut spans = Vec::new();
        let mut metrics = Vec::new();
        let mut logs = Vec::new();
        let mut resource: Option<ResourceIdentity> = None;
        for line in lines {
            let record: DiagnosticRecord =
                serde_json::from_str(line).map_err(|_| OtlpError::InvalidRecord)?;
            if resource.is_none() {
                resource = Some(record.resource.clone());
            }
            match record.signal {
                fluzo_core::telemetry::SignalKind::Span => {
                    if let Some(span) = proto_span(&record)? {
                        spans.push(span);
                    }
                }
                fluzo_core::telemetry::SignalKind::Metric => {
                    let DiagnosticPayload::Metric(snapshot) = &record.payload else {
                        return Err(OtlpError::InvalidRecord);
                    };
                    metrics.push(proto_metric(snapshot)?);
                }
                fluzo_core::telemetry::SignalKind::Log => {
                    if let Some(entry) = proto_log(&record)? {
                        logs.push(entry);
                    }
                }
            }
        }
        let resource = resource.unwrap_or(ResourceIdentity {
            service_name: "fluzo".into(),
            service_version: env!("CARGO_PKG_VERSION").into(),
        });
        let scope = proto::InstrumentationScope {
            name: "fluzo.runtime".into(),
            version: resource.service_version.clone(),
            attributes: Vec::new(),
            dropped_attributes_count: 0,
        };
        let resource_pb = proto::Resource {
            attributes: vec![
                proto::KeyValue {
                    key: "service.name".into(),
                    value: Some(any_string(resource.service_name.clone())),
                    ..Default::default()
                },
                proto::KeyValue {
                    key: "service.version".into(),
                    value: Some(any_string(resource.service_version.clone())),
                    ..Default::default()
                },
            ],
            entity_refs: Vec::new(),
            dropped_attributes_count: 0,
        };
        let mut requests = Vec::new();
        if !spans.is_empty() {
            let request = proto::ExportTraceServiceRequest {
                resource_spans: vec![proto::ResourceSpans {
                    resource: Some(resource_pb.clone()),
                    scope_spans: vec![proto::ScopeSpans {
                        scope: Some(scope.clone()),
                        spans,
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                }],
            };
            requests.push(("/v1/traces", request.encode_to_vec()));
        }
        if !metrics.is_empty() {
            let request = proto::ExportMetricsServiceRequest {
                resource_metrics: vec![proto::ResourceMetrics {
                    resource: Some(resource_pb.clone()),
                    scope_metrics: vec![proto::ScopeMetrics {
                        scope: Some(scope.clone()),
                        metrics,
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                }],
            };
            requests.push(("/v1/metrics", request.encode_to_vec()));
        }
        if !logs.is_empty() {
            let request = proto::ExportLogsServiceRequest {
                resource_logs: vec![proto::ResourceLogs {
                    resource: Some(resource_pb),
                    scope_logs: vec![proto::ScopeLogs {
                        scope: Some(scope),
                        log_records: logs,
                        schema_url: String::new(),
                    }],
                    schema_url: String::new(),
                }],
            };
            requests.push(("/v1/logs", request.encode_to_vec()));
        }
        Ok(requests)
    }
}

impl DiagnosticSink for OtlpHttpSink {
    fn name(&self) -> String {
        "otlp-http".into()
    }

    fn write_batch(&mut self, lines: &[String]) -> Result<(), String> {
        let requests = Self::build_requests(lines).map_err(|error| error.category())?;
        let target =
            Target::parse(&self.config.endpoint).ok_or(OtlpError::InvalidEndpoint.category())?;
        for (path, body) in requests {
            if body.len() > MAX_REQUEST_BYTES {
                return Err(OtlpError::Http(413).category());
            }
            post(&target, path, &body, self.config.timeout).map_err(|error| error.category())?;
        }
        Ok(())
    }
}

/// One bounded HTTP POST. Blocking std TCP keeps the sink on its hub
/// worker thread without owning an async runtime; the whole exchange
/// (connect, send, read) is bounded by the configured timeout via the
/// socket timeouts. Redirects are returned as an error, never
/// followed, and the response body is capped.
fn post(target: &Target, path: &str, body: &[u8], timeout: Duration) -> Result<(), OtlpError> {
    let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    let socket_timeout = Duration::from_millis(timeout_ms.max(1));
    let address = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(|_| OtlpError::Connection)?
        .next()
        .ok_or(OtlpError::Connection)?;
    let mut stream =
        TcpStream::connect_timeout(&address, socket_timeout).map_err(|_| OtlpError::Connection)?;
    stream
        .set_read_timeout(Some(socket_timeout))
        .map_err(|_| OtlpError::Connection)?;
    stream
        .set_write_timeout(Some(socket_timeout))
        .map_err(|_| OtlpError::Connection)?;
    let request_head = format!(
        "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        target.authority,
        body.len()
    );
    let started = Instant::now();
    stream
        .write_all(request_head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .map_err(|_| OtlpError::Connection)?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        if started.elapsed() > timeout {
            return Err(OtlpError::Timeout);
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                response.extend_from_slice(&buffer[..read]);
                if response.len() > MAX_RESPONSE_BYTES {
                    return Err(OtlpError::Http(502));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(OtlpError::Connection),
        }
    }
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(OtlpError::Http(502))?;
    let head = String::from_utf8_lossy(&response[..header_end]).to_string();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or(OtlpError::Http(502))?;
    if (200..300).contains(&status) {
        return Ok(());
    }
    if status == 429 {
        return Err(OtlpError::RateLimited);
    }
    if (300..400).contains(&status) {
        return Err(OtlpError::RedirectRejected);
    }
    Err(OtlpError::Http(status))
}

#[cfg(test)]
#[path = "telemetry_otlp_tests.rs"]
mod tests;
