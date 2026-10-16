//! AGT-02 P3: bounded, observable and reversible automatic compaction.
//!
//! PRD v0.2 section 16.1 and Architecture 11 allow compaction only under
//! conditions that are easy to state and easy to get wrong. This module
//! makes each one structural:
//!
//! * **Bounded.** The work is a fixed number of provider calls decided
//!   before the first one runs. A plan that would need more segments than
//!   [`MAX_COMPACTION_SEGMENTS`] is refused rather than allowed to loop.
//! * **Observable.** Every run ends in a durable journal event carrying
//!   what was replaced, what it cost and what the provider reported. A
//!   compaction that produced nothing visible did not happen.
//! * **Reversible.** The summary is an *additional* item with its own
//!   provenance. The items it stands in for are never deleted, and
//!   [`reverse`] reconstructs the pre-compaction context from the
//!   artifact's recorded coverage.
//! * **Non-promoting.** The summary is [`TrustTier::ModelText`]. It can
//!   never carry an authorization consequence, and the required items —
//!   runtime records, the user's request, applicable instructions — are
//!   preserved verbatim rather than summarized. PRD 16.1 forbids a summary
//!   from becoming a permission rule or standing in for runtime state.
//!
//! The provider call goes through the AGT-01 gateway as
//! [`RequestClass::Compaction`]: foreground work that pays the same
//! admission and budget costs as any other call, not an exemption.

use std::future::Future;
use std::pin::Pin;

use fluzo_core::admission::RequestClass;
use fluzo_core::context::{
    self, ContextBudget, ContextItem, ContextPack, Retention, SourceKind, SourceRef, TrustTier,
};
use fluzo_core::settings::ContextSettings;
use fluzo_core::telemetry::GenAiUsage;
use tokio::sync::watch;

use crate::store::{EventRecord, StateUpdate, Store};

/// State row kind for a committed compaction artifact.
pub const COMPACTION_STATE_KIND: &str = "compaction";
/// Journal event kind written when a compaction is applied.
pub const COMPACTION_EVENT_KIND: &str = "compaction_applied";

/// Hard ceiling on provider calls one compaction may make.
///
/// A bound, not a tuning knob: without it a context that keeps growing
/// during compaction could keep the engine looping indefinitely. Exceeding
/// it is a refusal the caller sees, not a retry.
pub const MAX_COMPACTION_SEGMENTS: u32 = 8;

/// Why a compaction did not run, or stopped.
///
/// A closed set so every non-run has a reason a caller can branch on and a
/// log can name. There is no `Other` and no silent give-up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompactionBlocked {
    /// Usage is below the configured threshold; nothing to do.
    NotNeeded,
    /// At the threshold, but no item is eligible to be summarized.
    NothingToCompact,
    /// The per-call input budget is zero, so no compaction call could carry
    /// anything. The reserves the model's window leaves are not enough to
    /// run compaction at all.
    NoSegmentCapacity,
    /// One item is too large to fit a single compaction call, so it could
    /// not be summarized without cutting it down first. Refused rather than
    /// silently truncated on the way into a lossy summary.
    SegmentOverCapacity {
        identity: String,
        tokens: u64,
        segment_budget: u64,
    },
    /// The eligible content needs more calls than [`MAX_COMPACTION_SEGMENTS`].
    SegmentLimitExceeded { segments: usize, maximum: u32 },
    /// The transport has no endpoint configured.
    NoEndpoint,
    /// The provider call failed. Carries the transport's own reason.
    ProviderFailed(String),
    /// A segment came back with no text. An empty summary would silently
    /// erase the segment's content, so the whole run is refused.
    EmptySummary { segment: u32 },
    /// The combined summary does not fit the space compaction was meant to
    /// free. Summarizing to this degree would save nothing.
    SummaryTooLarge { summary_tokens: u64, allowance: u64 },
    /// Cancellation was observed before a provider call was made.
    Cancelled,
    /// The durable commit failed. No artifact landed.
    Store(String),
}

impl std::fmt::Display for CompactionBlocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotNeeded => formatter.write_str("below the compaction threshold"),
            Self::NothingToCompact => {
                formatter.write_str("at the threshold but nothing is eligible to summarize")
            }
            Self::NoSegmentCapacity => {
                formatter.write_str("the window leaves no room for a compaction call")
            }
            Self::SegmentOverCapacity {
                identity,
                tokens,
                segment_budget,
            } => write!(
                formatter,
                "'{identity}' needs {tokens} tokens but one compaction call holds {segment_budget}"
            ),
            Self::SegmentLimitExceeded { segments, maximum } => {
                write!(
                    formatter,
                    "{segments} segments needed, maximum is {maximum}"
                )
            }
            Self::NoEndpoint => formatter.write_str("no compaction endpoint configured"),
            Self::ProviderFailed(reason) => write!(formatter, "provider call failed: {reason}"),
            Self::EmptySummary { segment } => {
                write!(formatter, "segment {segment} returned no summary text")
            }
            Self::SummaryTooLarge {
                summary_tokens,
                allowance,
            } => write!(
                formatter,
                "summary is {summary_tokens} tokens but only {allowance} were available"
            ),
            Self::Cancelled => formatter.write_str("cancelled before the next provider call"),
            Self::Store(reason) => write!(formatter, "durable commit failed: {reason}"),
        }
    }
}

/// One provider call's worth of work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionCall {
    pub segment: u32,
    pub segment_total: u32,
    pub class: RequestClass,
    pub items: Vec<ContextItem>,
    pub max_output_tokens: u64,
}

/// What one segment's provider call returned.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct SegmentSummary {
    pub text: String,
    /// Provider-reported usage. `None` means unknown and is never replaced
    /// by zeros: an estimate the engine made is not a provider measurement.
    pub usage: Option<GenAiUsage>,
}

/// The provider seam.
///
/// Dyn-compatible by returning a boxed future rather than pulling in a
/// dependency. `ProviderGateway::complete` backs the production
/// implementation; tests substitute a scripted one so the engine's
/// decisions — not the network — are what gets asserted.
pub trait CompactionTransport: Send + Sync {
    fn summarize<'a>(
        &'a self,
        call: &'a CompactionCall,
        cancel: &'a mut watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<SegmentSummary, CompactionBlocked>> + Send + 'a>>;
}

/// What a compaction will do, decided before any provider call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionPlan {
    /// Kept verbatim: required retention and runtime-authority content.
    pub preserved: Vec<ContextItem>,
    /// Replaced by the summary, in the order they will be sent.
    pub summarized: Vec<ContextItem>,
    /// The summarized items cut into per-call segments.
    pub segments: Vec<Vec<ContextItem>>,
    /// Tokens the summarized items occupy today.
    pub replaced_tokens: u64,
    /// Tokens one call may carry, after output and protocol reserves.
    pub segment_budget: u64,
}

/// Whether an item may be replaced by a summary.
///
/// Required items are never eligible: they are the constraints the task
/// runs under, and a lossy restatement of an instruction or a runtime
/// record is not the same thing. Model-produced content and optional
/// repository material are what compaction exists for.
pub fn is_compactable(item: &ContextItem) -> bool {
    if item.retention == Retention::Required {
        return false;
    }
    !matches!(item.source.kind, SourceKind::RuntimeRecord)
}

/// Input budget for one compaction call.
///
/// The compaction model window minus the summary's own output reserve and
/// protocol overhead. Deliberately derived from the same budget arithmetic
/// as normal turns so a compaction cannot be given more room than the
/// window actually has.
pub fn segment_budget(budget: &ContextBudget, settings: &ContextSettings) -> u64 {
    let output = settings.compaction_max_output_tokens;
    budget
        .model_window
        .saturating_sub(output)
        .saturating_sub(budget.protocol_overhead)
        .saturating_sub(budget.safety_reserve)
}

/// Decide the whole compaction before running any of it.
///
/// Deterministic: the same pack and settings give the same plan, including
/// segment boundaries.
pub fn plan(
    pack: &ContextPack,
    settings: &ContextSettings,
) -> Result<CompactionPlan, CompactionBlocked> {
    if !threshold_reached(pack, settings) {
        return Err(CompactionBlocked::NotNeeded);
    }
    let mut preserved = Vec::new();
    let mut summarized = Vec::new();
    for item in &pack.included {
        if is_compactable(item) {
            summarized.push(item.clone());
        } else {
            preserved.push(item.clone());
        }
    }
    if summarized.is_empty() {
        return Err(CompactionBlocked::NothingToCompact);
    }

    let segment_budget = segment_budget(&pack.budget, settings);
    if segment_budget == 0 {
        return Err(CompactionBlocked::NoSegmentCapacity);
    }
    // Whether the summary actually fits the space freed is settled after
    // the calls, by `SummaryTooLarge`. A pre-flight check on the preserved
    // total would be dead code: if anything is eligible to summarize, the
    // assembled pack already proved the preserved items fit alongside it.

    let mut segments: Vec<Vec<ContextItem>> = Vec::new();
    let mut current: Vec<ContextItem> = Vec::new();
    let mut current_tokens = 0u64;
    for item in summarized {
        let tokens = item.estimate.tokens;
        if tokens > segment_budget {
            return Err(CompactionBlocked::SegmentOverCapacity {
                identity: item.source.identity.clone(),
                tokens,
                segment_budget,
            });
        }
        if current_tokens + tokens > segment_budget && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        current_tokens += tokens;
        current.push(item);
    }
    if !current.is_empty() {
        segments.push(current);
    }

    if segments.len() > MAX_COMPACTION_SEGMENTS as usize {
        return Err(CompactionBlocked::SegmentLimitExceeded {
            segments: segments.len(),
            maximum: MAX_COMPACTION_SEGMENTS,
        });
    }

    let replaced_tokens = total_tokens(&segments.iter().flatten().cloned().collect::<Vec<_>>());
    Ok(CompactionPlan {
        preserved,
        summarized: segments.iter().flatten().cloned().collect(),
        segments,
        replaced_tokens,
        segment_budget,
    })
}

/// Whether usage has reached the configured automatic threshold.
pub fn threshold_reached(pack: &ContextPack, settings: &ContextSettings) -> bool {
    let effective = pack.budget.effective_input();
    if effective == 0 {
        return true;
    }
    let used = pack.estimated_input.tokens;
    let trigger = effective as f64 * settings.compaction_threshold;
    used as f64 >= trigger
}

/// The durable record of one applied compaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionArtifact {
    /// The combined summary text, in segment order.
    pub summary: String,
    /// Exactly what the summary stands in for. This is what makes reversal
    /// possible and what makes the summary auditable.
    pub covers: Vec<SourceRef>,
    pub segments: u32,
    pub replaced_tokens: u64,
    pub summary_estimate: fluzo_core::context::TokenEstimate,
    /// Provider-reported usage summed across segments. `None` when no
    /// segment reported anything.
    pub provider_usage: Option<GenAiUsage>,
    /// The workspace revision the summarized content was observed against,
    /// carried so a later reader can tell what the summary describes.
    pub observed_revision: Option<String>,
}

impl CompactionArtifact {
    /// The summary as a context item.
    ///
    /// Model text, optional retention, with a provenance ref that names the
    /// segments it covers. It is never required and never authoritative.
    pub fn as_context_item(&self) -> ContextItem {
        let identity = format!(
            "compaction:{}..{}",
            self.covers
                .first()
                .map(|s| s.identity.as_str())
                .unwrap_or("-"),
            self.covers
                .last()
                .map(|s| s.identity.as_str())
                .unwrap_or("-"),
        );
        ContextItem::new(
            SourceRef::new(SourceKind::CompactionSummary, identity)
                .with_version(self.summary_estimate.tokens.to_string()),
            TrustTier::ModelText,
            Retention::Optional,
            self.summary.clone(),
        )
    }

    pub fn state_update(&self, session_id: &str) -> StateUpdate {
        StateUpdate {
            id: format!("compaction:{session_id}"),
            kind: COMPACTION_STATE_KIND.to_owned(),
            payload: self.to_json(),
        }
    }

    pub fn event_record(&self, session_id: &str) -> EventRecord {
        EventRecord {
            state_id: format!("compaction:{session_id}"),
            kind: COMPACTION_EVENT_KIND.to_owned(),
            payload: self.to_json(),
        }
    }

    /// The observable payload. Every field a reader needs to answer "what
    /// did this compaction replace and what did it cost" is here, so the
    /// journal record alone is sufficient to audit the decision.
    fn to_json(&self) -> String {
        let covers: Vec<String> = self
            .covers
            .iter()
            .map(|source| format!("{:?}:{}", source.kind, source.identity))
            .collect();
        let usage = match self.provider_usage.as_ref() {
            Some(usage) => format!(
                "\"provider_usage\":{{\"input_tokens\":{},\"output_tokens\":{}}}",
                opt(usage.input_tokens),
                opt(usage.output_tokens)
            ),
            None => "\"provider_usage\":null".to_owned(),
        };
        format!(
            "{{\"segments\":{},\"covers\":[{}],\"replaced_tokens\":{},\"summary_tokens\":{},\"observed_revision\":{},{},",
            self.segments,
            covers
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(","),
            self.replaced_tokens,
            self.summary_estimate.tokens,
            match self.observed_revision.as_ref() {
                Some(rev) => format!("\"{rev}\""),
                None => "null".to_owned(),
            },
            usage
        )
    }
}

fn opt(value: Option<u64>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => "null".to_owned(),
    }
}

/// The context after compaction: preserved items plus the summary.
pub fn next_items(plan: &CompactionPlan, artifact: &CompactionArtifact) -> Vec<ContextItem> {
    let mut items = plan.preserved.clone();
    items.push(artifact.as_context_item());
    items
}

/// Reverse a compaction: drop the summary and put back what it covered.
///
/// Reversal is available because compaction never destroyed anything. The
/// originals are supplied from wherever they were retained; the artifact
/// names them, so the operation is exact rather than approximate.
pub fn reverse(
    current: &[ContextItem],
    originals: &[ContextItem],
    artifact: &CompactionArtifact,
) -> Vec<ContextItem> {
    let covered: Vec<&str> = artifact
        .covers
        .iter()
        .map(|s| s.identity.as_str())
        .collect();
    let mut restored: Vec<ContextItem> = current
        .iter()
        .filter(|item| item.source.kind != SourceKind::CompactionSummary)
        .cloned()
        .collect();
    for item in originals {
        if covered.contains(&item.source.identity.as_str()) {
            restored.push(item.clone());
        }
    }
    restored
}

/// The result of a run that completed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactionOutcome {
    pub artifact: CompactionArtifact,
    /// Items after compaction, in plan order.
    pub items: Vec<ContextItem>,
    /// Net tokens freed: replaced minus the summary's own cost.
    pub net_freed_tokens: u64,
}

/// Run the compaction: plan, call the provider per segment, build the
/// artifact.
///
/// Cancellation is checked *before* each provider call, never during, so
/// the number of calls that actually left the process is always known from
/// the outcome.
pub async fn run(
    pack: &ContextPack,
    settings: &ContextSettings,
    transport: &dyn CompactionTransport,
    cancel: &mut watch::Receiver<bool>,
) -> Result<CompactionOutcome, CompactionBlocked> {
    let plan = plan(pack, settings)?;
    let total = plan.segments.len() as u32;

    let mut texts: Vec<String> = Vec::with_capacity(plan.segments.len());
    let mut usage_total = GenAiUsage::default();
    let mut any_usage = false;

    for (index, segment) in plan.segments.iter().enumerate() {
        if *cancel.borrow() {
            return Err(CompactionBlocked::Cancelled);
        }
        let call = CompactionCall {
            segment: index as u32,
            segment_total: total,
            class: RequestClass::Compaction,
            items: segment.clone(),
            max_output_tokens: settings.compaction_max_output_tokens,
        };
        let summary = transport.summarize(&call, cancel).await?;
        if summary.text.trim().is_empty() {
            return Err(CompactionBlocked::EmptySummary {
                segment: index as u32,
            });
        }
        if let Some(usage) = summary.usage.as_ref() {
            usage_total.input_tokens = add(usage_total.input_tokens, usage.input_tokens);
            usage_total.output_tokens = add(usage_total.output_tokens, usage.output_tokens);
            any_usage = true;
        }
        texts.push(summary.text);
    }

    let summary = texts.join("\n");
    let summary_estimate = context::estimate_tokens(&summary);
    let preserved_tokens = total_tokens(&plan.preserved);
    let allowance = pack
        .budget
        .effective_input()
        .saturating_sub(preserved_tokens);
    if summary_estimate.tokens > allowance {
        return Err(CompactionBlocked::SummaryTooLarge {
            summary_tokens: summary_estimate.tokens,
            allowance,
        });
    }

    let artifact = CompactionArtifact {
        summary,
        covers: plan.summarized.iter().map(|i| i.source.clone()).collect(),
        segments: total,
        replaced_tokens: plan.replaced_tokens,
        summary_estimate,
        provider_usage: any_usage.then_some(usage_total),
        observed_revision: observed_revision(pack),
    };
    let items = next_items(&plan, &artifact);
    let net_freed_tokens = plan.replaced_tokens.saturating_sub(summary_estimate.tokens);
    Ok(CompactionOutcome {
        artifact,
        items,
        net_freed_tokens,
    })
}

/// Persist an artifact as one atomic unit.
///
/// One `BEGIN IMMEDIATE` transaction carries the state row and the journal
/// event, so a failure leaves no half-written compaction behind. The
/// caller learns the run did not land and the pre-compaction context stays
/// the live one.
pub async fn commit(
    store: &mut Store,
    session_id: &str,
    artifact: &CompactionArtifact,
) -> Result<(), CompactionBlocked> {
    store
        .commit_unit(
            &[artifact.state_update(session_id)],
            &[artifact.event_record(session_id)],
        )
        .await
        .map_err(|error| CompactionBlocked::Store(format!("{error:?}")))
}

/// Read back a committed artifact's raw payload, if any.
pub async fn committed_payload(store: &mut Store, session_id: &str) -> Option<String> {
    store
        .state(&format!("compaction:{session_id}"))
        .await
        .ok()
        .flatten()
        .map(|(_kind, payload, _revision)| payload)
}

/// The workspace revision the pack's runtime record refers to.
fn observed_revision(pack: &ContextPack) -> Option<String> {
    pack.included
        .iter()
        .find(|item| item.source.kind == SourceKind::RuntimeRecord)
        .and_then(|item| item.source.observed_version.clone())
}

fn total_tokens(items: &[ContextItem]) -> u64 {
    items.iter().map(|item| item.estimate.tokens).sum()
}

fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}
