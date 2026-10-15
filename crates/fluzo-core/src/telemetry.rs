//! OBS-01 telemetry DTOs and pure token-accounting policy.
//!
//! Three-path diagnostics per Architecture A09: the SQLite audit path
//! (runtime `telemetry_audit`), the bounded local JSONL diagnostic
//! archive (runtime `telemetry_archive`) and the optional OTLP export
//! path (deferred; see the OBS-01 plan) share this record vocabulary and
//! correlation identifiers but not their failure semantics.
//!
//! GenAI usage follows the OpenTelemetry GenAI semantic conventions
//! pinned at revision `8ffdf568e1b4391a99adb081db16e8102e36918e` of
//! the GenAI conventions repository. Unknown usage values are omitted,
//! never zero-filled, and cache/reasoning subsets are never summed on top
//! of the input/output totals.

use serde::{Deserialize, Serialize};

/// Local diagnostic record envelope version. Rotation of this version is
/// a documented format change, not an SDK default.
pub const TELEMETRY_FORMAT_VERSION: u32 = 1;

/// Pinned GenAI semantic-conventions revision used by these names.
/// Upgrading requires explicit review; never mix revisions.
pub const GENAI_CONVENTIONS_REVISION: &str = "8ffdf568e1b4391a99adb081db16e8102e36918e";

/// Token unit used by the pinned GenAI token instruments.
pub const TOKEN_UNIT: &str = "{token}";
/// Duration unit used by the pinned GenAI duration instruments.
pub const SECONDS_UNIT: &str = "s";

/// Pinned GenAI instrument names (section 13.3).
pub mod instruments {
    pub const USAGE_INPUT_TOKENS: &str = "gen_ai.client.inference.usage.input_tokens";
    pub const USAGE_OUTPUT_TOKENS: &str = "gen_ai.client.inference.usage.output_tokens";
    pub const USAGE_CACHE_READ_INPUT_TOKENS: &str =
        "gen_ai.client.inference.usage.cache_read.input_tokens";
    pub const USAGE_CACHE_WRITE_INPUT_TOKENS: &str =
        "gen_ai.client.inference.usage.cache_write.input_tokens";
    pub const USAGE_REASONING_OUTPUT_TOKENS: &str =
        "gen_ai.client.inference.usage.reasoning.output_tokens";
    pub const OPERATION_INPUT_TOKENS: &str = "gen_ai.client.inference.operation.input_tokens";
    pub const OPERATION_OUTPUT_TOKENS: &str = "gen_ai.client.inference.operation.output_tokens";
    pub const OPERATION_DURATION: &str = "gen_ai.client.operation.duration";
    pub const OPERATION_TIME_TO_FIRST_CHUNK: &str = "gen_ai.client.operation.time_to_first_chunk";
}

/// Pinned GenAI attribute names (section 13.3). These are the standard
/// keys the GenAI conventions revision above defines; Fluzo-specific
/// metadata uses `fluzo.*` and never an invented `gen_ai.*` key.
pub mod attributes {
    pub const OPERATION_NAME: &str = "gen_ai.operation.name";
    pub const PROVIDER_NAME: &str = "gen_ai.provider.name";
    pub const REQUEST_MODEL: &str = "gen_ai.request.model";
    pub const RESPONSE_MODEL: &str = "gen_ai.response.model";
    pub const REQUEST_STREAM: &str = "gen_ai.request.stream";
    pub const REQUEST_MAX_TOKENS: &str = "gen_ai.request.max_tokens";
    pub const RESPONSE_ID: &str = "gen_ai.response.id";
    pub const RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
    pub const USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
    pub const USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";
    pub const USAGE_CACHE_READ_INPUT_TOKENS: &str = "gen_ai.usage.cache_read.input_tokens";
    pub const USAGE_CACHE_WRITE_INPUT_TOKENS: &str = "gen_ai.usage.cache_write.input_tokens";
    pub const USAGE_REASONING_OUTPUT_TOKENS: &str = "gen_ai.usage.reasoning.output_tokens";
    pub const RESPONSE_TIME_TO_FIRST_CHUNK: &str = "gen_ai.response.time_to_first_chunk";
    /// Required dimension on the inference usage counters: `text` when
    /// reliably known, otherwise `unknown`. Never omitted and never
    /// guessed from the model name.
    pub const TOKEN_MODALITY: &str = "gen_ai.token.modality";
    /// The existing real session identifier, when applicable. A second
    /// conversation id is never invented from a trace or hash.
    pub const CONVERSATION_ID: &str = "gen_ai.conversation.id";
    pub const ERROR_TYPE: &str = "error.type";
    /// Fluzo inference purpose (for example `agent_turn`,
    /// `context_compaction`, `capability_probe`). A standard field is
    /// never repurposed to mean a Fluzo phase.
    pub const FLUZO_INFERENCE_PURPOSE: &str = "fluzo.inference.purpose";

    /// Prompt/response content attributes. Absent by default: they
    /// require the existing explicit capture consent and redaction,
    /// and their absence never blocks token or latency instrumentation.
    pub const ABSENT_BY_DEFAULT: [&str; 3] = [
        "gen_ai.input.messages",
        "gen_ai.output.messages",
        "gen_ai.system_instructions",
    ];
}

/// `gen_ai.token.modality` values. Unknown is explicit, not a fallback
/// derived from the requested model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TokenModality {
    Text,
    #[default]
    Unknown,
}

impl TokenModality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Unknown => "unknown",
        }
    }
}

/// The pinned GenAI span name: `{gen_ai.operation.name} {gen_ai.request.model}`,
/// for example `chat flash-halogen`. Exactly one CLIENT span per logical
/// generative provider operation; never one per streamed chunk.
pub fn gen_ai_span_name(operation_name: &str, request_model: &str) -> String {
    format!("{operation_name} {request_model}")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SignalKind {
    Log,
    Span,
    Metric,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceIdentity {
    pub service_name: String,
    pub service_version: String,
}

/// Correlation identifiers shared by all three paths. Opaque hex strings;
/// IDs belong on records, never on metric dimensions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TraceRef {
    pub trace_id: String,
    pub span_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LogPayload {
    pub level: LogLevel,
    pub message: String,
    pub attributes: std::collections::BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SpanKind {
    Client,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SpanStatus {
    Ok,
    Error { kind: String },
}

/// Completed-span metadata. `sampled` records whether the span would be
/// exported by the (deferred) OTLP path; metric accounting is
/// independent of this flag.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SpanRecord {
    pub name: String,
    pub kind: SpanKind,
    pub start_unix_ms: i64,
    pub end_unix_ms: i64,
    pub status: SpanStatus,
    pub attributes: std::collections::BTreeMap<String, String>,
    pub sampled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum MetricKind {
    Counter,
    Histogram,
}

/// Metric temporality carried by every snapshot (section 13.2 requires
/// instrument type, unit, aggregation window *and* temporality). This is
/// declared explicitly rather than inherited from an SDK default, so a
/// later OTLP increment cannot silently change what a stored snapshot
/// means.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum MetricTemporality {
    /// Change since the previous export for this stream.
    #[default]
    Delta,
    /// Monotonically accumulated total since process start.
    Cumulative,
}

/// SDK-style aggregate snapshot. These are aggregates over a window, not
/// per-token samples and not counters reconstructed from sampled spans.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricAggregation {
    Sum {
        value: f64,
    },
    Histogram {
        count: u64,
        sum: f64,
        bounds: Vec<f64>,
        bucket_counts: Vec<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricSnapshot {
    pub name: String,
    pub unit: String,
    pub kind: MetricKind,
    pub temporality: MetricTemporality,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    /// Bounded configured dimensions only. Task/session IDs, paths, raw
    /// error strings and prompts are never dimensions.
    pub dimensions: std::collections::BTreeMap<String, String>,
    pub aggregation: MetricAggregation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DiagnosticPayload {
    Log(LogPayload),
    Span(SpanRecord),
    Metric(MetricSnapshot),
}

/// Versioned local diagnostic envelope (section 13.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticRecord {
    pub format_version: u32,
    pub signal: SignalKind,
    pub timestamp_ms: i64,
    pub resource: ResourceIdentity,
    pub trace: Option<TraceRef>,
    pub payload: DiagnosticPayload,
}

impl DiagnosticRecord {
    pub fn new(
        signal: SignalKind,
        timestamp_ms: i64,
        resource: ResourceIdentity,
        trace: Option<TraceRef>,
        payload: DiagnosticPayload,
    ) -> Self {
        Self {
            format_version: TELEMETRY_FORMAT_VERSION,
            signal,
            timestamp_ms,
            resource,
            trace,
            payload,
        }
    }
}

/// Reported/normalized GenAI usage. `None` means unknown and is omitted
/// from records; it is never written as zero. Cache and reasoning fields
/// are subsets already contained in the input/output totals.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenAiUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
}

impl GenAiUsage {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Provenance of one usage ledger entry. Final-response usage, known
/// physical-retry consumption and incomplete totals stay distinct.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UsageSource {
    FinalResponse,
    PhysicalRetry,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UsageLedgerEntry {
    pub source: UsageSource,
    pub usage: GenAiUsage,
}

/// Raw provider-reported usage as surfaced by an adapter before
/// normalization. `cumulative` marks reports that restate the running
/// total of the logical operation (repeated cumulative chunks) rather
/// than a fresh increment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsageReport {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub cumulative: bool,
}

/// Once-per-operation normalizer (section 13.3): repeated cumulative
/// reports are deduplicated against the last observed cumulative total,
/// a usage-only final chunk is honored before finalization, and unknown
/// values are omitted rather than zeroed. The gateway calls this once;
/// spans, metrics and the ledger consume the normalized result so no
/// layer adds usage a second time.
#[derive(Clone, Debug, Default)]
pub struct UsageNormalizer {
    last_cumulative: Option<GenAiUsage>,
    known: GenAiUsage,
    retry_known: GenAiUsage,
}

fn genai_from(report: &ProviderUsageReport) -> GenAiUsage {
    GenAiUsage {
        input_tokens: report.input_tokens,
        output_tokens: report.output_tokens,
        cache_read_input_tokens: report.cache_read_input_tokens,
        cache_write_input_tokens: report.cache_write_input_tokens,
        reasoning_output_tokens: report.reasoning_output_tokens,
    }
}

fn merge_known(known: GenAiUsage, reported: GenAiUsage) -> GenAiUsage {
    GenAiUsage {
        input_tokens: match (known.input_tokens, reported.input_tokens) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
        output_tokens: match (known.output_tokens, reported.output_tokens) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
        cache_read_input_tokens: match (
            known.cache_read_input_tokens,
            reported.cache_read_input_tokens,
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
        cache_write_input_tokens: match (
            known.cache_write_input_tokens,
            reported.cache_write_input_tokens,
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
        reasoning_output_tokens: match (
            known.reasoning_output_tokens,
            reported.reasoning_output_tokens,
        ) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        },
    }
}

impl UsageNormalizer {
    /// Feeds one provider report. Returns the newly accounted (delta)
    /// usage for this report: zero-valued fields where nothing new is
    /// known. Repeated identical cumulative reports yield no new usage.
    pub fn report(&mut self, report: ProviderUsageReport) -> UsageLedgerEntry {
        let reported = genai_from(&report);
        if report.cumulative {
            if let Some(previous) = &self.last_cumulative {
                // Deduplicate: only fields that grew are new consumption.
                let delta = delta_usage(previous, &reported);
                self.last_cumulative = Some(merge_known(*previous, reported));
                self.known = merge_known(self.known, reported);
                return UsageLedgerEntry {
                    source: UsageSource::FinalResponse,
                    usage: delta,
                };
            }
            self.last_cumulative = Some(reported);
            self.known = merge_known(self.known, reported);
            return UsageLedgerEntry {
                source: UsageSource::FinalResponse,
                usage: reported,
            };
        }
        // Non-cumulative (incremental) reports add directly.
        self.known = add_usage(self.known, reported);
        UsageLedgerEntry {
            source: UsageSource::FinalResponse,
            usage: reported,
        }
    }

    /// Marks the operation as ending without a complete usage report.
    /// Whatever was known remains recorded; the remainder is explicitly
    /// incomplete rather than guessed. Retry consumption is tracked
    /// separately and is not folded in here.
    pub fn finish_incomplete(&mut self) -> Option<UsageLedgerEntry> {
        if self.known.is_empty() {
            None
        } else {
            Some(UsageLedgerEntry {
                source: UsageSource::Incomplete,
                usage: self.known,
            })
        }
    }

    /// Feeds one physical-retry report. A task execution attempt and a
    /// provider transport retry are different identities: retry
    /// consumption is recorded once, labeled `PhysicalRetry`, and is
    /// never deduplicated against the logical operation's cumulative
    /// total. Keeping it out of the cumulative stream is what stops a
    /// retry from being read as restated final usage, and stops a parent
    /// span, child HTTP span or resumed subscription from adding it a
    /// second time.
    pub fn report_retry(&mut self, report: ProviderUsageReport) -> UsageLedgerEntry {
        let reported = genai_from(&report);
        self.retry_known = add_usage(self.retry_known, reported);
        UsageLedgerEntry {
            source: UsageSource::PhysicalRetry,
            usage: reported,
        }
    }

    /// Known final-response usage for the logical operation.
    pub fn known(&self) -> GenAiUsage {
        self.known
    }

    /// Accumulated known physical-retry consumption, kept distinct from
    /// final-response usage.
    pub fn retry_known(&self) -> GenAiUsage {
        self.retry_known
    }

    /// Total known consumption across both identities. Callers that
    /// need the operation total use this rather than summing ledger
    /// entries ad hoc, so the distinction stays intact.
    pub fn total_known(&self) -> GenAiUsage {
        add_usage(self.known, self.retry_known)
    }
}

fn delta_usage(previous: &GenAiUsage, current: &GenAiUsage) -> GenAiUsage {
    let field = |a: Option<u64>, b: Option<u64>| match (a, b) {
        (Some(a), Some(b)) if b > a => Some(b - a),
        (None, Some(b)) => Some(b),
        _ => None,
    };
    GenAiUsage {
        input_tokens: field(previous.input_tokens, current.input_tokens),
        output_tokens: field(previous.output_tokens, current.output_tokens),
        cache_read_input_tokens: field(
            previous.cache_read_input_tokens,
            current.cache_read_input_tokens,
        ),
        cache_write_input_tokens: field(
            previous.cache_write_input_tokens,
            current.cache_write_input_tokens,
        ),
        reasoning_output_tokens: field(
            previous.reasoning_output_tokens,
            current.reasoning_output_tokens,
        ),
    }
}

fn add_usage(base: GenAiUsage, extra: GenAiUsage) -> GenAiUsage {
    GenAiUsage {
        input_tokens: match (base.input_tokens, extra.input_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
        output_tokens: match (base.output_tokens, extra.output_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
        cache_read_input_tokens: match (base.cache_read_input_tokens, extra.cache_read_input_tokens)
        {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
        cache_write_input_tokens: match (
            base.cache_write_input_tokens,
            extra.cache_write_input_tokens,
        ) {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
        reasoning_output_tokens: match (base.reasoning_output_tokens, extra.reasoning_output_tokens)
        {
            (Some(a), Some(b)) => Some(a + b),
            (a, b) => a.or(b),
        },
    }
}

/// Required SQLite audit record. Committed by the runtime audit path in
/// the same durable unit as the work it authorizes or records; it is
/// never produced by the diagnostic sinks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuditCategory {
    PermissionDecision,
    IntentCommit,
    OutcomeCommit,
    ConfigurationChange,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: String,
    pub category: AuditCategory,
    pub subject: String,
    pub action: String,
    pub outcome: String,
    pub correlation_id: String,
    pub detail: String,
    pub created_unix_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cumulative(input: Option<u64>, output: Option<u64>) -> ProviderUsageReport {
        ProviderUsageReport {
            input_tokens: input,
            output_tokens: output,
            cumulative: true,
            ..Default::default()
        }
    }

    #[test]
    fn repeated_cumulative_reports_add_no_new_usage() {
        let mut normalizer = UsageNormalizer::default();
        let first = normalizer.report(cumulative(Some(10), Some(5)));
        assert_eq!(first.usage.input_tokens, Some(10));
        assert_eq!(first.usage.output_tokens, Some(5));
        let repeat = normalizer.report(cumulative(Some(10), Some(5)));
        assert!(
            repeat.usage.is_empty(),
            "identical cumulative restates nothing new"
        );
        assert_eq!(
            normalizer.known(),
            GenAiUsage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                ..Default::default()
            }
        );
    }

    #[test]
    fn growing_cumulative_reports_yield_only_the_delta() {
        let mut normalizer = UsageNormalizer::default();
        normalizer.report(cumulative(Some(10), Some(5)));
        let delta = normalizer.report(cumulative(Some(10), Some(8)));
        assert_eq!(delta.usage.input_tokens, None);
        assert_eq!(delta.usage.output_tokens, Some(3));
        assert_eq!(normalizer.known().output_tokens, Some(8));
    }

    #[test]
    fn usage_only_final_chunk_is_honored_before_finalization() {
        let mut normalizer = UsageNormalizer::default();
        // Stream chunks without usage contribute nothing.
        let quiet = normalizer.report(ProviderUsageReport::default());
        assert!(quiet.usage.is_empty());
        // Final usage-only chunk carries the whole operation's usage.
        let final_chunk = normalizer.report(cumulative(Some(100), Some(20)));
        assert_eq!(final_chunk.usage.input_tokens, Some(100));
        assert_eq!(final_chunk.usage.output_tokens, Some(20));
    }

    #[test]
    fn missing_counts_are_omitted_not_zeroed() {
        let mut normalizer = UsageNormalizer::default();
        let entry = normalizer.report(ProviderUsageReport {
            input_tokens: Some(42),
            ..Default::default()
        });
        assert_eq!(entry.usage.input_tokens, Some(42));
        assert_eq!(entry.usage.output_tokens, None);
        assert_eq!(entry.usage.reasoning_output_tokens, None);
    }

    #[test]
    fn cache_and_reasoning_subsets_never_join_the_totals() {
        // The pinned example: 100 in (40 cached) + 20 out (5 reasoning)
        // consumes 120 tokens total, not 165.
        let usage = GenAiUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_input_tokens: Some(40),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(5),
        };
        let total = usage.input_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0);
        assert_eq!(total, 120);
        // Normalizing a cumulative report with subsets keeps the input
        // total at the reported value; subsets are recorded alongside.
        let mut normalizer = UsageNormalizer::default();
        let entry = normalizer.report(ProviderUsageReport {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_input_tokens: Some(40),
            cache_write_input_tokens: None,
            reasoning_output_tokens: Some(5),
            cumulative: true,
        });
        assert_eq!(entry.usage.input_tokens, Some(100));
        assert_eq!(entry.usage.cache_read_input_tokens, Some(40));
        assert_eq!(entry.usage.reasoning_output_tokens, Some(5));
    }

    #[test]
    fn ledger_keeps_retry_and_final_usage_distinct() {
        let entries = [
            UsageLedgerEntry {
                source: UsageSource::PhysicalRetry,
                usage: GenAiUsage {
                    input_tokens: Some(30),
                    output_tokens: Some(2),
                    ..Default::default()
                },
            },
            UsageLedgerEntry {
                source: UsageSource::FinalResponse,
                usage: GenAiUsage {
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                    cache_read_input_tokens: Some(40),
                    ..Default::default()
                },
            },
        ];
        let input: u64 = entries.iter().filter_map(|e| e.usage.input_tokens).sum();
        let output: u64 = entries.iter().filter_map(|e| e.usage.output_tokens).sum();
        assert_eq!((input, output), (130, 22));
        // Subsets never inflate totals and retry identity is preserved
        // without creating duplicate GenAI usage records.
        assert_eq!(entries[1].usage.cache_read_input_tokens, Some(40));
        assert_eq!(entries[0].source, UsageSource::PhysicalRetry);
    }

    #[test]
    fn incomplete_operations_keep_known_usage_and_mark_the_gap() {
        let mut normalizer = UsageNormalizer::default();
        normalizer.report(cumulative(Some(7), None));
        let incomplete = normalizer.finish_incomplete().unwrap();
        assert_eq!(incomplete.source, UsageSource::Incomplete);
        assert_eq!(incomplete.usage.input_tokens, Some(7));
        let mut empty = UsageNormalizer::default();
        assert!(empty.finish_incomplete().is_none());
    }

    #[test]
    fn pinned_instrument_names_and_units_match_the_conventions_revision() {
        assert_eq!(
            GENAI_CONVENTIONS_REVISION,
            "8ffdf568e1b4391a99adb081db16e8102e36918e"
        );
        assert_eq!(TOKEN_UNIT, "{token}");
        assert_eq!(SECONDS_UNIT, "s");
        assert_eq!(
            instruments::USAGE_INPUT_TOKENS,
            "gen_ai.client.inference.usage.input_tokens"
        );
        assert_eq!(
            instruments::USAGE_REASONING_OUTPUT_TOKENS,
            "gen_ai.client.inference.usage.reasoning.output_tokens"
        );
        assert_eq!(
            instruments::OPERATION_INPUT_TOKENS,
            "gen_ai.client.inference.operation.input_tokens"
        );
        assert_eq!(
            instruments::OPERATION_DURATION,
            "gen_ai.client.operation.duration"
        );
        assert_eq!(
            instruments::OPERATION_TIME_TO_FIRST_CHUNK,
            "gen_ai.client.operation.time_to_first_chunk"
        );
    }

    #[test]
    fn pinned_gen_ai_attribute_names_match_the_conventions_revision() {
        assert_eq!(attributes::OPERATION_NAME, "gen_ai.operation.name");
        assert_eq!(attributes::REQUEST_MODEL, "gen_ai.request.model");
        assert_eq!(attributes::USAGE_INPUT_TOKENS, "gen_ai.usage.input_tokens");
        assert_eq!(
            attributes::USAGE_REASONING_OUTPUT_TOKENS,
            "gen_ai.usage.reasoning.output_tokens"
        );
        assert_eq!(attributes::TOKEN_MODALITY, "gen_ai.token.modality");
        assert_eq!(attributes::CONVERSATION_ID, "gen_ai.conversation.id");
        assert_eq!(attributes::ERROR_TYPE, "error.type");
        assert_eq!(
            attributes::FLUZO_INFERENCE_PURPOSE,
            "fluzo.inference.purpose"
        );
        // Content attributes are the absent-by-default set, listed so a
        // capture policy can assert they are withheld rather than
        // assuming nothing ever emits them.
        assert_eq!(
            attributes::ABSENT_BY_DEFAULT,
            [
                "gen_ai.input.messages",
                "gen_ai.output.messages",
                "gen_ai.system_instructions"
            ]
        );
        // Fluzo metadata never invents a gen_ai.* key.
        assert!(attributes::FLUZO_INFERENCE_PURPOSE.starts_with("fluzo."));
    }

    #[test]
    fn token_modality_is_explicit_and_never_guessed() {
        assert_eq!(TokenModality::Text.as_str(), "text");
        assert_eq!(TokenModality::Unknown.as_str(), "unknown");
        // Unknown is the default: a missing modality is reported as
        // unknown rather than inferred from the requested model name.
        assert_eq!(TokenModality::default(), TokenModality::Unknown);
    }

    #[test]
    fn gen_ai_span_name_follows_the_pinned_operation_model_pattern() {
        assert_eq!(
            gen_ai_span_name("chat", "flash-halogen"),
            "chat flash-halogen"
        );
        // One span per logical provider operation; the name carries the
        // exact model identifier sent over the API, not the config key.
        assert_eq!(gen_ai_span_name("chat", "coder"), "chat coder");
    }

    #[test]
    fn metric_snapshot_declares_its_temporality_explicitly() {
        // Section 13.2 requires instrument type, unit, aggregation window
        // and temporality on every stored metric snapshot. Temporality is
        // an explicit field rather than an inherited SDK default, so a
        // stored snapshot keeps meaning the same thing once the OTLP
        // increment lands. JSON round trips of this envelope are covered
        // in the runtime suite, where serde_json is an allowed dependency.
        for temporality in [MetricTemporality::Delta, MetricTemporality::Cumulative] {
            let snapshot = MetricSnapshot {
                name: instruments::USAGE_INPUT_TOKENS.to_string(),
                unit: TOKEN_UNIT.to_string(),
                kind: MetricKind::Counter,
                temporality,
                window_start_ms: 1_700_000_000_000,
                window_end_ms: 1_700_000_001_000,
                dimensions: std::collections::BTreeMap::from([(
                    attributes::TOKEN_MODALITY.to_string(),
                    TokenModality::Text.as_str().to_string(),
                )]),
                aggregation: MetricAggregation::Sum { value: 120.0 },
            };
            assert_eq!(snapshot.temporality, temporality);
            assert_eq!(snapshot.unit, "{token}");
            assert_eq!(
                snapshot
                    .dimensions
                    .get(attributes::TOKEN_MODALITY)
                    .map(String::as_str),
                Some("text")
            );
            assert_eq!(snapshot.kind, MetricKind::Counter);
        }
        assert_eq!(MetricTemporality::default(), MetricTemporality::Delta);
    }

    #[test]
    fn retry_consumption_stays_distinct_from_final_response_usage() {
        let mut normalizer = UsageNormalizer::default();
        // An allowed automatic physical retry that reported usage before failing.
        let retry = normalizer.report_retry(ProviderUsageReport {
            input_tokens: Some(30),
            output_tokens: Some(2),
            ..Default::default()
        });
        assert_eq!(retry.source, UsageSource::PhysicalRetry);
        // The successful attempt reports the logical operation's cumulative usage.
        let final_entry = normalizer.report(cumulative(Some(100), Some(20)));
        assert_eq!(final_entry.source, UsageSource::FinalResponse);
        assert_eq!(final_entry.usage.input_tokens, Some(100));

        // The retry never entered the cumulative dedup stream: a restated
        // cumulative report still adds nothing new.
        let restated = normalizer.report(cumulative(Some(100), Some(20)));
        assert!(restated.usage.is_empty());

        // The two identities remain separately readable.
        assert_eq!(
            normalizer.known(),
            GenAiUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                ..Default::default()
            }
        );
        assert_eq!(
            normalizer.retry_known(),
            GenAiUsage {
                input_tokens: Some(30),
                output_tokens: Some(2),
                ..Default::default()
            }
        );
        // Total is the operation plus its retries, counted once each.
        assert_eq!(normalizer.total_known().input_tokens, Some(130));
        assert_eq!(normalizer.total_known().output_tokens, Some(22));
    }

    #[test]
    fn repeated_retries_accumulate_without_touching_final_usage() {
        let mut normalizer = UsageNormalizer::default();
        for _ in 0..3 {
            normalizer.report_retry(ProviderUsageReport {
                input_tokens: Some(10),
                output_tokens: Some(1),
                ..Default::default()
            });
        }
        assert_eq!(normalizer.retry_known().input_tokens, Some(30));
        assert_eq!(normalizer.retry_known().output_tokens, Some(3));
        // No final-response usage was reported, so the logical total stays
        // empty and is not silently filled from retry consumption.
        assert!(normalizer.known().is_empty());
        assert!(normalizer.finish_incomplete().is_none());
    }

    #[test]
    fn metric_observations_survive_a_sampled_out_span() {
        // Section 13.3: metric observations are independent of whether
        // the corresponding span is sampled. A record built from an
        // unsampled span's operation must still carry its metric.
        let unsampled_span = SpanRecord {
            name: gen_ai_span_name("chat", "flash-halogen"),
            kind: SpanKind::Client,
            start_unix_ms: 1_700_000_000_000,
            end_unix_ms: 1_700_000_001_500,
            status: SpanStatus::Ok,
            attributes: std::collections::BTreeMap::from([(
                attributes::USAGE_INPUT_TOKENS.to_string(),
                "100".to_string(),
            )]),
            sampled: false,
        };
        let metric = MetricSnapshot {
            name: instruments::USAGE_INPUT_TOKENS.to_string(),
            unit: TOKEN_UNIT.to_string(),
            kind: MetricKind::Counter,
            temporality: MetricTemporality::Delta,
            window_start_ms: unsampled_span.start_unix_ms,
            window_end_ms: unsampled_span.end_unix_ms,
            dimensions: std::collections::BTreeMap::from([(
                attributes::TOKEN_MODALITY.to_string(),
                TokenModality::Unknown.as_str().to_string(),
            )]),
            aggregation: MetricAggregation::Sum { value: 100.0 },
        };
        assert!(!unsampled_span.sampled);
        // The metric is its own record and does not derive from the span's
        // sampling decision.
        let window_end_ms = metric.window_end_ms;
        let record = DiagnosticRecord::new(
            SignalKind::Metric,
            window_end_ms,
            ResourceIdentity {
                service_name: "fluzo".into(),
                service_version: "0.0.1".into(),
            },
            None,
            DiagnosticPayload::Metric(metric),
        );
        assert_eq!(record.signal, SignalKind::Metric);
        assert_eq!(record.format_version, TELEMETRY_FORMAT_VERSION);
        match &record.payload {
            DiagnosticPayload::Metric(m) => {
                assert_eq!(m.aggregation, MetricAggregation::Sum { value: 100.0 });
                assert_eq!(m.kind, MetricKind::Counter);
                assert_eq!(m.temporality, MetricTemporality::Delta);
            }
            other => panic!("expected a metric payload, got {other:?}"),
        }
    }
}
