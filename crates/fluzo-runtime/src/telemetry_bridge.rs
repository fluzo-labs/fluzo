//! OBS-01b tracing bridge: a `tracing` subscriber that publishes into
//! the existing [`DiagnosticHub`].
//!
//! The hub stays the single capture boundary: every event crossing this
//! bridge goes through the hub's one redaction pass and lands in each
//! sink's own bounded queue, exactly like records published directly
//! by runtime code. `tracing-opentelemetry` is deliberately NOT used:
//! it would route events into a separate OpenTelemetry SDK pipeline
//! with its own processors, bypassing hub redaction and creating a
//! second unredacted capture path, which the OBS-01 contract forbids.
//! The bridge is built on `tracing-core`, which is already in the
//! dependency graph, so no new package enters the workspace.
//!
//! Scope of the bridge: `tracing` events become hub Log records.
//! GenAI spans and metrics keep their explicit runtime publishers
//! (the provider gateway builds `SpanRecord`/`MetricSnapshot` from
//! normalized usage); the bridge never synthesizes span or metric
//! records from tracing metadata, so it cannot double-count usage.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, LogLevel, LogPayload, ResourceIdentity, SignalKind,
};
use tracing_core::field::{Field, Visit};
use tracing_core::span::{self, Id};
use tracing_core::{Event, Level, Subscriber};

use crate::telemetry::DiagnosticHub;

/// Collects the message and structured fields of one tracing event.
#[derive(Default)]
struct EventFields {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl Visit for EventFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            // `tracing` renders the formatted message; drop the
            // debug quotes so the hub sees the plain text.
            self.message = Some(rendered.trim_matches('"').to_owned());
        } else {
            self.fields.insert(field.name().to_owned(), rendered);
        }
    }
}

fn level_of(level: &Level) -> LogLevel {
    match *level {
        Level::TRACE | Level::DEBUG => LogLevel::Debug,
        Level::INFO => LogLevel::Info,
        Level::WARN => LogLevel::Warn,
        Level::ERROR => LogLevel::Error,
    }
}

/// A `tracing` subscriber that forwards events to the hub and treats
/// spans as inert placeholders. Span records with real GenAI semantics
/// come from the runtime instrumentation, not from this bridge.
pub struct HubTracingSubscriber {
    hub: Arc<DiagnosticHub>,
    resource: ResourceIdentity,
    next_span: AtomicU64,
}

impl HubTracingSubscriber {
    pub fn new(hub: Arc<DiagnosticHub>, resource: ResourceIdentity) -> Self {
        Self {
            hub,
            resource,
            next_span: AtomicU64::new(1),
        }
    }
}

impl Subscriber for HubTracingSubscriber {
    fn enabled(&self, _metadata: &tracing_core::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &span::Attributes<'_>) -> Id {
        // Inert placeholder ids: the bridge does not model spans.
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&self, _span: &Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut visitor = EventFields::default();
        event.record(&mut visitor);
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as i64)
            .unwrap_or(0);
        let message = visitor
            .message
            .unwrap_or_else(|| event.metadata().target().to_owned());
        // The hub performs the single redaction pass on the way in;
        // this bridge never sees or stores unredacted state.
        self.hub.publish(DiagnosticRecord::new(
            SignalKind::Log,
            timestamp_ms,
            self.resource.clone(),
            None,
            DiagnosticPayload::Log(LogPayload {
                level: level_of(event.metadata().level()),
                message,
                attributes: visitor.fields,
            }),
        ));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{HubLimits, RedactionPolicy};
    use std::sync::Mutex;

    struct CapturingSink {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl crate::telemetry::DiagnosticSink for CapturingSink {
        fn name(&self) -> String {
            "capture".into()
        }
        fn write_batch(&mut self, lines: &[String]) -> Result<(), String> {
            self.lines.lock().unwrap().extend(lines.iter().cloned());
            Ok(())
        }
    }

    #[test]
    fn tracing_events_flow_through_the_hub_redaction_into_sinks() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let policy = RedactionPolicy {
            secret_values: vec!["hunter2".into()],
            ..RedactionPolicy::default()
        };
        let mut hub = DiagnosticHub::new(HubLimits::default(), policy);
        hub.add_sink(Box::new(CapturingSink {
            lines: Arc::clone(&captured),
        }));
        let subscriber = Arc::new(HubTracingSubscriber::new(
            Arc::new(hub),
            ResourceIdentity {
                service_name: "fluzo".into(),
                service_version: "0.1.0".into(),
            },
        ));
        let dispatch = tracing_core::Dispatch::new(subscriber);
        tracing_core::dispatcher::with_default(&dispatch, || {
            tracing::info!(target: "fluzo.runtime.test", "login used hunter2");
            tracing::warn!(target: "fluzo.runtime.test", attempt = 2, "retrying");
        });
        // The hub worker drains asynchronously; poll with a bounded
        // deadline instead of an arbitrary sleep.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if captured.lock().unwrap().len() >= 2 || std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let lines = captured.lock().unwrap().clone();
        assert_eq!(lines.len(), 2, "both events must reach the sink");
        assert!(
            lines[0].contains("[REDACTED]") && !lines[0].contains("hunter2"),
            "hub redaction applies to bridged events: {}",
            lines[0]
        );
        let second: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(second["signal"], "Log");
        assert_eq!(second["payload"]["Log"]["level"], "Warn");
        assert_eq!(
            second["payload"]["Log"]["attributes"]["attempt"], "2",
            "structured fields become string attributes"
        );
    }
}
