//! OBS-01b local receiver conformance: a loopback OTLP HTTP/protobuf
//! receiver (no Docker) proves each of the three signals independently,
//! the pinned GenAI names/units/dimensions on the wire, redaction on
//! exported bytes, the no-retry rules for 429/redirect, and bounded
//! collector-outage behaviour.
//!
//! The receiver is strict: it records every request with its raw body
//! so tests can assert both the decoded protobuf and the byte-level
//! absence of secrets. It only ever listens on 127.0.0.1.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, LogLevel, LogPayload, MetricAggregation, MetricKind,
    MetricTemporality, ResourceIdentity, SignalKind, SpanKind, SpanRecord, SpanStatus, TraceRef,
    attributes, instruments,
};
use fluzo_runtime::telemetry::{DiagnosticHub, HubLimits, RedactionPolicy};
use fluzo_runtime::telemetry_otlp::{OtlpExportConfig, OtlpHttpSink};
use prost::Message as _;

#[derive(Clone, Debug)]
struct Received {
    path: String,
    content_type: String,
    body: Vec<u8>,
}

/// Receiver behaviour modes.
const MODE_OK: u8 = 0;
const MODE_RESET: u8 = 1; // accept and immediately close (connection reset)

/// A strict loopback OTLP receiver. Unexpected connections are still
/// recorded; nothing is silently ignored.
struct Receiver {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<Received>>>,
    mode: Arc<AtomicU8>,
    status: Arc<AtomicU16>,
    location: Arc<Mutex<Option<String>>>,
    shutdown: Arc<AtomicU8>,
    handle: Option<JoinHandle<()>>,
}

impl Receiver {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback receiver");
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(AtomicU8::new(MODE_OK));
        let status = Arc::new(AtomicU16::new(200));
        let location = Arc::new(Mutex::new(None));
        let shutdown = Arc::new(AtomicU8::new(0));
        let handle = {
            let requests = Arc::clone(&requests);
            let mode = Arc::clone(&mode);
            let status = Arc::clone(&status);
            let location = Arc::clone(&location);
            let shutdown = Arc::clone(&shutdown);
            std::thread::spawn(move || {
                let _ = listener.set_nonblocking(true);
                loop {
                    if shutdown.load(Ordering::Relaxed) != 0 {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _peer)) => {
                            let mode = mode.load(Ordering::Relaxed);
                            let status = status.load(Ordering::Relaxed);
                            let location = location.lock().unwrap().clone();
                            let requests = Arc::clone(&requests);
                            std::thread::spawn(move || {
                                let _ = stream.set_nonblocking(false);
                                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                                if mode == MODE_RESET {
                                    let _ = stream.shutdown(std::net::Shutdown::Both);
                                    return;
                                }
                                serve(stream, status, location, requests);
                            });
                        }
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                || error.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => return,
                    }
                }
            })
        };
        Self {
            addr,
            requests,
            mode,
            status,
            location,
            shutdown,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn set_status(&self, status: u16) {
        self.status.store(status, Ordering::Relaxed);
    }

    fn set_redirect(&self, location: &str) {
        self.set_status(302);
        *self.location.lock().unwrap() = Some(location.to_owned());
    }

    fn set_reset(&self) {
        self.mode.store(MODE_RESET, Ordering::Relaxed);
    }

    fn set_ok(&self) {
        self.mode.store(MODE_OK, Ordering::Relaxed);
        self.status.store(200, Ordering::Relaxed);
    }

    fn requests(&self) -> Vec<Received> {
        self.requests.lock().unwrap().clone()
    }

    fn wait_for_requests(&self, count: usize) -> Vec<Received> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let requests = self.requests();
            if requests.len() >= count || Instant::now() > deadline {
                return requests;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.shutdown.store(1, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(
    mut stream: TcpStream,
    status: u16,
    location: Option<String>,
    requests: Arc<Mutex<Vec<Received>>>,
) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(position) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break position;
                }
                if buffer.len() > 8 * 1024 * 1024 {
                    return;
                }
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let mut content_length = 0usize;
    let mut content_type = String::new();
    for line in lines {
        let lower = line.to_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = lower.strip_prefix("content-type:") {
            content_type = value.trim().to_owned();
        }
    }
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
            Err(_) => break,
        }
    }
    // Receiver-side strictness: only the three OTLP paths are valid.
    assert!(
        path == "/v1/traces" || path == "/v1/metrics" || path == "/v1/logs",
        "unexpected OTLP path: {path}"
    );
    // Record before responding so the test can observe the request even
    // when the response fails.
    requests.lock().unwrap().push(Received {
        path,
        content_type,
        body,
    });
    let reason = if status == 429 {
        "Too Many Requests"
    } else {
        "OK"
    };
    let mut response = format!("HTTP/1.1 {status} {reason}\r\nContent-Length: 0\r\n");
    if let Some(location) = location {
        response.push_str(&format!("Location: {location}\r\n"));
    }
    response.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

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
            name: fluzo_core::telemetry::gen_ai_span_name("chat", "flash-halogen"),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_500,
            status: SpanStatus::Ok,
            attributes: std::collections::BTreeMap::from([
                (attributes::OPERATION_NAME.into(), "chat".into()),
                (attributes::REQUEST_MODEL.into(), "flash-halogen".into()),
                (attributes::USAGE_INPUT_TOKENS.into(), "100".into()),
            ]),
            sampled,
        }),
    )
}

fn counter_record(name: &str, value: f64) -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Metric,
        1_700_000_001_000,
        resource(),
        None,
        DiagnosticPayload::Metric(fluzo_core::telemetry::MetricSnapshot {
            name: name.into(),
            unit: fluzo_core::telemetry::TOKEN_UNIT.into(),
            kind: MetricKind::Counter,
            temporality: MetricTemporality::Delta,
            window_start_ms: 1_700_000_000_000,
            window_end_ms: 1_700_000_001_000,
            dimensions: std::collections::BTreeMap::from([(
                attributes::TOKEN_MODALITY.into(),
                "unknown".into(),
            )]),
            aggregation: MetricAggregation::Sum { value },
        }),
    )
}

fn log_record(level: LogLevel, message: &str) -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_002_000,
        resource(),
        Some(trace()),
        DiagnosticPayload::Log(LogPayload {
            level,
            message: message.into(),
            attributes: std::collections::BTreeMap::from([(
                attributes::FLUZO_INFERENCE_PURPOSE.into(),
                "agent_turn".into(),
            )]),
        }),
    )
}

fn hub_with_receiver(receiver: &Receiver) -> DiagnosticHub {
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(Box::new(OtlpHttpSink::new(OtlpExportConfig {
        endpoint: receiver.endpoint(),
        timeout: Duration::from_secs(5),
        batch_records: 64,
    })));
    hub
}

#[test]
fn each_signal_is_proven_independently_at_the_receiver() {
    let receiver = Receiver::start();
    let hub = hub_with_receiver(&receiver);
    // A trace bridge does not prove log or metric delivery: publish
    // one of each and require three separate OTLP requests.
    let span = hub.publish(span_record(true));
    let metric = hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 100.0));
    let log = hub.publish(log_record(LogLevel::Info, "turn finished"));
    assert_eq!(span.delivered, 1);
    assert_eq!(metric.delivered, 1);
    assert_eq!(log.delivered, 1);
    let requests = receiver.wait_for_requests(3);
    let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    assert!(paths.contains(&"/v1/traces"));
    assert!(paths.contains(&"/v1/metrics"));
    assert!(paths.contains(&"/v1/logs"));
    for request in &requests {
        assert_eq!(request.content_type, "application/x-protobuf");
    }
}

#[test]
fn pinned_genai_names_units_and_dimensions_survive_the_wire() {
    let receiver = Receiver::start();
    let hub = hub_with_receiver(&receiver);
    hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 100.0));
    hub.publish(counter_record(
        instruments::USAGE_REASONING_OUTPUT_TOKENS,
        5.0,
    ));
    // Batching may split the two publishes across requests; collect
    // names across every metrics request until both are visible.
    let mut names: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for request in receiver.wait_for_requests(1) {
            if request.path != "/v1/metrics" {
                continue;
            }
            let metrics =
                opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(
                    request.body.as_slice(),
                )
                .unwrap();
            for metric in metrics.resource_metrics[0].scope_metrics[0].metrics.iter() {
                if !names.contains(&metric.name) {
                    names.push(metric.name.clone());
                }
            }
        }
        if names.contains(&instruments::USAGE_INPUT_TOKENS.to_string())
            && names.contains(&instruments::USAGE_REASONING_OUTPUT_TOKENS.to_string())
            || Instant::now() > deadline
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(names.contains(&instruments::USAGE_INPUT_TOKENS.to_string()));
    assert!(names.contains(&instruments::USAGE_REASONING_OUTPUT_TOKENS.to_string()));
    for request in receiver.requests() {
        if request.path != "/v1/metrics" {
            continue;
        }
        let metrics =
            opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(
                request.body.as_slice(),
            )
            .unwrap();
        for metric in metrics.resource_metrics[0].scope_metrics[0].metrics.iter() {
            assert_eq!(metric.unit, "{token}");
            match metric.data.as_ref().unwrap() {
                opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum) => {
                    assert_eq!(
                        sum.aggregation_temporality,
                        opentelemetry_proto::tonic::metrics::v1::AggregationTemporality::Delta
                            as i32
                    );
                    let point = &sum.data_points[0];
                    let modalities: Vec<&str> = point
                    .attributes
                    .iter()
                    .filter(|kv| kv.key == attributes::TOKEN_MODALITY)
                    .map(|kv| match kv.value.as_ref().unwrap().value.as_ref().unwrap() {
                        opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => {
                            s.as_str()
                        }
                        _ => panic!("modality must be a string"),
                    })
                    .collect();
                    assert_eq!(modalities, vec!["unknown"]);
                }
                _ => panic!("expected sum"),
            }
        }
    }
}

#[test]
fn no_duplicate_usage_from_span_and_metric_instrumentation() {
    // One operation: one span carrying usage attributes plus one
    // normalized metric record. The receiver must see the 100 input
    // tokens exactly once as a metric data point; the span never
    // becomes a second metric.
    let receiver = Receiver::start();
    let hub = hub_with_receiver(&receiver);
    hub.publish(span_record(true));
    hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 100.0));
    let requests = receiver.wait_for_requests(2);
    let mut metric_total = 0.0;
    let mut span_count = 0;
    for request in &requests {
        if request.path == "/v1/metrics" {
            let metrics =
                opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(
                    request.body.as_slice(),
                )
                .unwrap();
            for metric in metrics.resource_metrics[0].scope_metrics[0].metrics.iter() {
                if let opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum) =
                    metric.data.as_ref().unwrap()
                {
                    for point in &sum.data_points {
                        if let opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(v) =
                            point.value.unwrap()
                        {
                            metric_total += v;
                        }
                    }
                }
            }
        }
        if request.path == "/v1/traces" {
            let traces =
                opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(
                    request.body.as_slice(),
                )
                .unwrap();
            span_count += traces.resource_spans[0].scope_spans[0].spans.len();
        }
    }
    assert_eq!(span_count, 1);
    assert_eq!(metric_total, 100.0, "usage must be accounted exactly once");
}

#[test]
fn redaction_is_proven_on_exported_bytes_not_only_locally() {
    let receiver = Receiver::start();
    let policy = RedactionPolicy {
        secret_values: vec!["hunter2".into()],
        ..RedactionPolicy::default()
    };
    let mut hub = DiagnosticHub::new(HubLimits::default(), policy);
    hub.add_sink(Box::new(OtlpHttpSink::new(OtlpExportConfig {
        endpoint: receiver.endpoint(),
        timeout: Duration::from_secs(5),
        batch_records: 64,
    })));
    let mut record = log_record(LogLevel::Info, "login with hunter2 failed");
    if let DiagnosticPayload::Log(log) = &mut record.payload {
        log.attributes
            .insert("authorization".into(), "Bearer hunter2".into());
        // Even if a producer tried, content attributes must not reach
        // the wire by default.
        log.attributes
            .insert("gen_ai.input.messages".into(), "secret prompt".into());
    }
    hub.publish(record);
    let requests = receiver.wait_for_requests(1);
    let raw = &requests[0].body;
    let haystack = String::from_utf8_lossy(raw).to_string();
    assert!(!haystack.contains("hunter2"), "secret leaked on the wire");
    assert!(haystack.contains("[REDACTED]"), "redaction marker missing");
    let logs = opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest::decode(
        raw.as_slice(),
    )
    .unwrap();
    let log = &logs.resource_logs[0].scope_logs[0].log_records[0];
    match log.body.as_ref().unwrap().value.as_ref().unwrap() {
        opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(message) => {
            assert_eq!(message, "login with [REDACTED] failed")
        }
        _ => panic!("expected string body"),
    }
    let keys: Vec<&str> = log.attributes.iter().map(|kv| kv.key.as_str()).collect();
    assert!(keys.contains(&"authorization"));
    for attribute in &log.attributes {
        if attribute.key == "authorization" {
            match attribute.value.as_ref().unwrap().value.as_ref().unwrap() {
                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value) => {
                    assert_eq!(value, "[REDACTED]")
                }
                _ => panic!("expected string"),
            }
        }
    }
}

#[test]
fn content_attributes_are_absent_by_default() {
    // The default pipeline never emits content attributes; the export
    // path must not invent them either.
    let receiver = Receiver::start();
    let hub = hub_with_receiver(&receiver);
    hub.publish(span_record(true));
    hub.publish(log_record(LogLevel::Info, "plain"));
    let requests = receiver.wait_for_requests(2);
    let haystack = requests
        .iter()
        .flat_map(|r| r.body.iter().copied())
        .collect::<Vec<u8>>();
    let text = String::from_utf8_lossy(&haystack).to_string();
    for forbidden in attributes::ABSENT_BY_DEFAULT {
        assert!(
            !text.contains(forbidden),
            "{forbidden} must be absent by default"
        );
    }
}

#[test]
fn rate_limit_yields_exactly_one_request_and_no_retry() {
    let receiver = Receiver::start();
    receiver.set_status(429);
    let hub = hub_with_receiver(&receiver);
    let outcome = hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 1.0));
    assert_eq!(outcome.delivered, 1, "hub accepted the record");
    receiver.wait_for_requests(1);
    // Give any (forbidden) retry a chance to show up.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        receiver.requests().len(),
        1,
        "429 must not trigger automatic retry, cooldown or fallback"
    );
    let health = hub.health();
    let sink = &health[0];
    assert!(sink.degraded, "429 must mark the export sink degraded");
    assert_eq!(sink.dropped.get("write_failed"), Some(&1));
    assert_eq!(sink.last_error.as_deref(), Some("rate_limited"));
}

#[test]
fn redirect_is_rejected_and_its_target_is_never_contacted() {
    let target = Receiver::start();
    let redirector = Receiver::start();
    redirector.set_redirect(&target.endpoint());
    let hub = hub_with_receiver(&redirector);
    hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 1.0));
    let requests = redirector.wait_for_requests(1);
    assert_eq!(requests.len(), 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        target.requests().len(),
        0,
        "the redirect target must receive zero requests"
    );
    let health = hub.health();
    assert_eq!(health[0].last_error.as_deref(), Some("redirect_rejected"));
}

#[test]
fn collector_outage_bounds_resources_and_new_signals_recover() {
    let receiver = Receiver::start();
    receiver.set_reset();
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(Box::new(OtlpHttpSink::new(OtlpExportConfig {
        endpoint: receiver.endpoint(),
        timeout: Duration::from_secs(2),
        batch_records: 64,
    })));
    // Outage: publishing must stay fast (non-blocking) and every loss
    // must be a counted, visible gap.
    let started = Instant::now();
    let mut dropped_gaps = 0usize;
    for index in 0..2_000u32 {
        let outcome = hub.publish(counter_record(
            instruments::USAGE_INPUT_TOKENS,
            f64::from(index),
        ));
        dropped_gaps += outcome.dropped;
    }
    let publish_elapsed = started.elapsed();
    let _ = dropped_gaps; // queue absorption vs producer drops is asserted via hub health below
    assert!(
        publish_elapsed < Duration::from_secs(5),
        "producer must not block on a failing collector: {publish_elapsed:?}"
    );
    // Wait for the sink worker to churn through the outage.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let health = &hub.health()[0];
        let queued = health.queued_records;
        let counted = health.dropped.values().sum::<u64>();
        if queued == 0 && counted >= 2_000 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "outage churn did not settle: {:?}",
            hub.health()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let health = &hub.health()[0];
    assert!(health.degraded);
    assert!(health.queued_records <= health.capacity_records);
    // Recovery: new signals are delivered after the collector returns.
    // Dropped records are NOT replayed; only new work flows.
    receiver.set_ok();
    hub.publish(counter_record(instruments::USAGE_OUTPUT_TOKENS, 42.0));
    let requests = receiver.wait_for_requests(1);
    let metrics =
        opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::decode(
            requests[0].body.as_slice(),
        )
        .unwrap();
    let names: Vec<String> = metrics.resource_metrics[0].scope_metrics[0]
        .metrics
        .iter()
        .map(|m| m.name.clone())
        .collect();
    assert_eq!(names, vec![instruments::USAGE_OUTPUT_TOKENS]);
    let total: u64 = requests.len().try_into().unwrap();
    assert_eq!(
        total, 1,
        "only the new signal is delivered, no silent replay"
    );
}

#[test]
fn shutdown_flush_drains_the_export_sink() {
    let receiver = Receiver::start();
    let mut hub = hub_with_receiver(&receiver);
    hub.publish(counter_record(instruments::USAGE_INPUT_TOKENS, 7.0));
    let report = hub.shutdown(Duration::from_secs(10));
    assert!(
        report.drained_sinks.contains(&"otlp-http".to_string()),
        "export sink must drain within the deadline: {report:?}"
    );
    assert!(report.incomplete_flushes.is_empty());
    let requests = receiver.wait_for_requests(1);
    assert_eq!(requests.len(), 1);
}

#[test]
fn producer_cost_stays_bounded_with_export_disabled_healthy_and_degraded() {
    const RECORDS: u32 = 2_000;
    // (a) Export disabled: no sink, no serialization beyond the hub's
    // own JSON envelope.
    let plain = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    let started = Instant::now();
    for index in 0..RECORDS {
        plain.publish(counter_record(
            instruments::USAGE_INPUT_TOKENS,
            f64::from(index),
        ));
    }
    let disabled = started.elapsed();
    assert!(
        disabled < Duration::from_secs(5),
        "disabled-path producer: {disabled:?}"
    );

    // (b) Healthy export sink.
    let receiver = Receiver::start();
    let healthy = hub_with_receiver(&receiver);
    let started = Instant::now();
    for index in 0..RECORDS {
        healthy.publish(counter_record(
            instruments::USAGE_INPUT_TOKENS,
            f64::from(index),
        ));
    }
    let healthy_publish = started.elapsed();
    assert!(
        healthy_publish < Duration::from_secs(5),
        "healthy export producer must stay non-blocking: {healthy_publish:?}"
    );
    let high_water = healthy
        .health()
        .iter()
        .map(|h| h.queued_records)
        .max()
        .unwrap_or(0);
    assert!(
        high_water <= 1024,
        "queue high-water {high_water} exceeds bound"
    );

    // (c) Degraded sink: same producer budget, counted gaps instead of
    // blocking.
    let down = Receiver::start();
    down.set_reset();
    let degraded = hub_with_receiver(&down);
    let started = Instant::now();
    for index in 0..RECORDS {
        degraded.publish(counter_record(
            instruments::USAGE_INPUT_TOKENS,
            f64::from(index),
        ));
    }
    let degraded_publish = started.elapsed();
    assert!(
        degraded_publish < Duration::from_secs(5),
        "degraded export producer must stay non-blocking: {degraded_publish:?}"
    );
    // Report high-water marks and gaps for the delivery record.
    let _ = (disabled, healthy_publish, degraded_publish, high_water);
}
