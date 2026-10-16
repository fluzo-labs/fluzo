//! AGT-02 P3: compaction engine tests (trigger, planning, running).
//!
//! The engine runs against a scripted transport so what is asserted is the
//! engine's decisions — whether it ran, how many calls it made, what it
//! refused and why — rather than network behaviour.

use std::future::Future;
use std::pin::Pin;

use fluzo_core::admission::RequestClass;
use fluzo_core::context::{
    ContextBudget, ContextItem, ContextPack, Retention, SourceKind, SourceRef, TrustTier, assemble,
};
use fluzo_core::settings::ContextSettings;
use fluzo_core::telemetry::GenAiUsage;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;

use crate::compaction::*;

pub(crate) fn runtime() -> &'static Runtime {
    static RT: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| Builder::new_current_thread().enable_all().build().unwrap())
}

/// Exactly `tokens` tokens: the estimator is characters / 4 rounded up.
pub(crate) fn text_for_tokens(tokens: u64) -> String {
    "x".repeat(tokens as usize * 4)
}

pub(crate) fn turn(index: u32, tokens: u64) -> ContextItem {
    ContextItem::new(
        SourceRef::new(SourceKind::PriorMessage, format!("turn:{index}")),
        TrustTier::ModelText,
        Retention::Optional,
        text_for_tokens(tokens),
    )
}

/// Window 1000, output 100, protocol 100, safety 100 -> 700 effective.
pub(crate) fn budget() -> ContextBudget {
    ContextBudget::new(1_000, 100, 100, 100)
}

/// Threshold 0.8, compaction output reserve 200 -> segment budget 600.
pub(crate) fn settings() -> ContextSettings {
    ContextSettings {
        compaction_threshold: 0.8,
        compaction_max_output_tokens: 200,
        ..ContextSettings::default()
    }
}

/// Three required items (10 tokens) plus `turns` 100-token turns.
///
/// Six turns put the pack at 610 of 700 effective input, past the 0.8
/// threshold: the situation compaction exists for.
pub(crate) fn pack_with(turns: u32) -> ContextPack {
    let mut items = vec![
        ContextItem::new(
            SourceRef::new(SourceKind::RuntimeRecord, "workspace:revision").with_version("rev-abc"),
            TrustTier::RuntimeAuthority,
            Retention::Required,
            "observed at rev-abc",
        ),
        ContextItem::new(
            SourceRef::new(SourceKind::UserRequest, "task:t1"),
            TrustTier::UserIntent,
            Retention::Required,
            "do the thing",
        ),
        ContextItem::new(
            SourceRef::new(SourceKind::InstructionFile, "AGENTS.md"),
            TrustTier::Instruction,
            Retention::Required,
            "rules",
        ),
    ];
    for index in 0..turns {
        items.push(turn(index, 100));
    }
    assemble(budget(), items).expect("pack assembles")
}

pub(crate) fn no_cancel() -> watch::Receiver<bool> {
    let (_sender, receiver) = watch::channel(false);
    receiver
}

// ---------------------------------------------------------------------------
// Scripted transport
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) enum Behavior {
    /// A short summary per segment, with provider-reported usage.
    Summarize,
    /// A short summary, no usage reported at all.
    NoUsage,
    /// The provider fails on this segment index.
    FailAt(u32),
    /// This segment returns whitespace only.
    EmptyAt(u32),
    /// Every segment returns a summary far larger than the space available.
    Huge,
}

pub(crate) struct Fake {
    behavior: Behavior,
    calls: std::sync::Mutex<Vec<CompactionCall>>,
}

impl Fake {
    pub(crate) fn new(behavior: Behavior) -> Self {
        Self {
            behavior,
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
    pub(crate) fn calls(&self) -> Vec<u32> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.segment)
            .collect()
    }
}

impl CompactionTransport for Fake {
    fn summarize<'a>(
        &'a self,
        call: &'a CompactionCall,
        _cancel: &'a mut watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<SegmentSummary, CompactionBlocked>> + Send + 'a>> {
        self.calls.lock().unwrap().push(call.clone());
        let outcome = match &self.behavior {
            Behavior::Summarize => Ok(SegmentSummary {
                text: format!("summary of segment {}", call.segment),
                usage: Some(GenAiUsage {
                    input_tokens: Some(120),
                    output_tokens: Some(11),
                    ..Default::default()
                }),
            }),
            Behavior::FailAt(index) if *index == call.segment => Err(
                CompactionBlocked::ProviderFailed("transport closed".to_owned()),
            ),
            Behavior::EmptyAt(index) if *index == call.segment => Ok(SegmentSummary {
                text: "   \n ".to_owned(),
                usage: None,
            }),
            Behavior::Huge => Ok(SegmentSummary {
                text: text_for_tokens(800),
                usage: None,
            }),
            _ => Ok(SegmentSummary {
                text: format!("summary of segment {}", call.segment),
                usage: None,
            }),
        };
        Box::pin(async move { outcome })
    }
}

// ---------------------------------------------------------------------------
// Trigger
// ---------------------------------------------------------------------------

#[test]
fn below_the_threshold_nothing_is_triggered() {
    let pack = pack_with(2); // 207 of 700
    assert!(!threshold_reached(&pack, &settings()));
    assert_eq!(plan(&pack, &settings()), Err(CompactionBlocked::NotNeeded));
}

#[test]
fn at_the_threshold_a_plan_is_produced() {
    let pack = pack_with(6); // 607 of 700, trigger at 560
    assert!(threshold_reached(&pack, &settings()));
    assert!(plan(&pack, &settings()).is_ok());
}

#[test]
fn required_only_content_at_the_threshold_has_nothing_to_compact() {
    let items = vec![ContextItem::new(
        SourceRef::new(SourceKind::RuntimeRecord, "workspace:revision"),
        TrustTier::RuntimeAuthority,
        Retention::Required,
        text_for_tokens(700),
    )];
    let pack = assemble(budget(), items).expect("pack");
    assert!(threshold_reached(&pack, &settings()));
    assert_eq!(
        plan(&pack, &settings()),
        Err(CompactionBlocked::NothingToCompact)
    );
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

#[test]
fn required_and_runtime_items_are_never_compactable() {
    let instruction = ContextItem::new(
        SourceRef::new(SourceKind::InstructionFile, "AGENTS.md"),
        TrustTier::Instruction,
        Retention::Required,
        "rules".to_owned(),
    );
    assert!(
        !is_compactable(&instruction),
        "an instruction is not summary fodder"
    );

    let runtime_record = ContextItem::new(
        SourceRef::new(SourceKind::RuntimeRecord, "budget"),
        TrustTier::RuntimeAuthority,
        Retention::Optional,
        "tokens left".to_owned(),
    );
    assert!(
        !is_compactable(&runtime_record),
        "a runtime record is never replaced, even marked optional"
    );

    let model_text = ContextItem::new(
        SourceRef::new(SourceKind::PriorMessage, "turn:1"),
        TrustTier::ModelText,
        Retention::Optional,
        "chat".to_owned(),
    );
    assert!(is_compactable(&model_text));
}

#[test]
fn planning_is_deterministic() {
    let pack = pack_with(8);
    let settings = settings();
    let first = plan(&pack, &settings).expect("plan");
    for _ in 0..5 {
        assert_eq!(plan(&pack, &settings).expect("plan"), first);
    }
}

#[test]
fn segments_respect_the_per_call_budget() {
    let settings = settings();
    let pack = pack_with(6);
    let plan = plan(&pack, &settings).expect("plan");
    assert_eq!(plan.segment_budget, 600);
    for segment in &plan.segments {
        let tokens: u64 = segment.iter().map(|i| i.estimate.tokens).sum();
        assert!(
            tokens <= plan.segment_budget,
            "segment over budget: {tokens}"
        );
    }
}

#[test]
fn a_multi_segment_plan_covers_every_eligible_item_once() {
    // Segment budget 550 against 600 eligible tokens forces two calls.
    let mut settings = settings();
    settings.compaction_max_output_tokens = 350;
    let pack = pack_with(6);
    let plan = plan(&pack, &settings).expect("plan");
    assert_eq!(plan.segments.len(), 2);
    let covered: Vec<&str> = plan
        .summarized
        .iter()
        .map(|i| i.source.identity.as_str())
        .collect();
    assert_eq!(
        covered,
        vec!["turn:0", "turn:1", "turn:2", "turn:3", "turn:4", "turn:5"]
    );
    assert_eq!(plan.replaced_tokens, 600);
}

#[test]
fn an_item_larger_than_one_call_is_refused_not_silently_truncated() {
    let mut settings = settings();
    settings.compaction_max_output_tokens = 750; // segment budget 50
    // Required content large enough to cross the threshold, so the plan
    // actually reaches the segmentation step.
    let items = vec![
        ContextItem::new(
            SourceRef::new(SourceKind::RuntimeRecord, "workspace:revision"),
            TrustTier::RuntimeAuthority,
            Retention::Required,
            text_for_tokens(550),
        ),
        turn(1, 60),
    ];
    let pack = assemble(budget(), items).expect("pack");
    assert_eq!(
        plan(&pack, &settings),
        Err(CompactionBlocked::SegmentOverCapacity {
            identity: "turn:1".to_owned(),
            tokens: 60,
            segment_budget: 50,
        })
    );
}

#[test]
fn a_plan_needing_too_many_calls_is_refused() {
    let mut settings = settings();
    settings.compaction_max_output_tokens = 750; // segment budget 50
    // 1 x required + 12 x 50 = 601 of 700 effective: past the threshold,
    // and one item per segment means twelve calls against a ceiling of 8.
    let mut items = vec![ContextItem::new(
        SourceRef::new(SourceKind::RuntimeRecord, "workspace:revision"),
        TrustTier::RuntimeAuthority,
        Retention::Required,
        "rev",
    )];
    for index in 0..12 {
        items.push(turn(index, 50));
    }
    let pack = assemble(budget(), items).expect("pack");
    match plan(&pack, &settings) {
        Err(CompactionBlocked::SegmentLimitExceeded { segments, maximum }) => {
            assert_eq!(maximum, MAX_COMPACTION_SEGMENTS);
            assert_eq!(segments, 12);
        }
        other => panic!("expected a segment-limit refusal, got {other:?}"),
    }
}

#[test]
fn a_window_with_no_room_for_a_compaction_call_is_refused() {
    // 1000 window - 800 compaction output - 100 protocol - 100 safety = 0.
    let mut settings = settings();
    settings.compaction_max_output_tokens = 800;
    let pack = pack_with(6);
    assert_eq!(
        plan(&pack, &settings),
        Err(CompactionBlocked::NoSegmentCapacity)
    );
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

#[test]
fn a_run_replaces_eligible_content_with_one_summary_item() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(&pack, &settings(), &transport, &mut no_cancel()))
        .expect("compaction runs");

    assert_eq!(transport.calls(), vec![0], "one segment, one call");
    assert_eq!(outcome.artifact.segments, 1);
    assert_eq!(outcome.artifact.covers.len(), 6);
    assert_eq!(outcome.artifact.replaced_tokens, 600);
    assert!(outcome.net_freed_tokens > 0);

    let summary = outcome
        .items
        .iter()
        .find(|i| i.source.kind == SourceKind::CompactionSummary)
        .expect("summary item present");
    assert_eq!(summary.tier, TrustTier::ModelText);
    assert_eq!(summary.retention, Retention::Optional);
    assert!(!summary.tier.can_authorize());

    for identity in ["workspace:revision", "task:t1", "AGENTS.md"] {
        let kept = outcome
            .items
            .iter()
            .find(|i| i.source.identity == identity)
            .unwrap_or_else(|| panic!("{identity} must survive"));
        let original = pack
            .included
            .iter()
            .find(|i| i.source.identity == identity)
            .expect("original");
        assert_eq!(kept.content, original.content, "{identity} unchanged");
    }
}

#[test]
fn the_run_never_leaves_authority_in_summarized_content() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(&pack, &settings(), &transport, &mut no_cancel()))
        .expect("compaction runs");
    let authoritative: Vec<&str> = outcome
        .items
        .iter()
        .filter(|i| i.tier.can_authorize())
        .map(|i| i.source.identity.as_str())
        .collect();
    assert_eq!(
        authoritative,
        vec!["workspace:revision"],
        "only the runtime record grants; never the summary"
    );
}

#[test]
fn every_call_is_classified_as_compaction_work() {
    let mut settings = settings();
    settings.compaction_max_output_tokens = 350; // two segments
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    runtime()
        .block_on(run(&pack, &settings, &transport, &mut no_cancel()))
        .expect("compaction runs");
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    for call in calls.iter() {
        assert_eq!(call.class, RequestClass::Compaction);
        assert_eq!(
            call.max_output_tokens,
            settings.compaction_max_output_tokens
        );
        assert_eq!(call.segment_total, 2);
    }
}

#[test]
fn a_provider_failure_stops_the_run_and_names_the_segment() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::FailAt(0));
    assert_eq!(
        runtime().block_on(run(&pack, &settings(), &transport, &mut no_cancel())),
        Err(CompactionBlocked::ProviderFailed(
            "transport closed".to_owned()
        ))
    );
    assert_eq!(transport.calls(), vec![0], "stopped at the failing segment");
}

#[test]
fn an_empty_summary_is_refused_rather_than_erasing_the_segment() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::EmptyAt(0));
    assert_eq!(
        runtime().block_on(run(&pack, &settings(), &transport, &mut no_cancel())),
        Err(CompactionBlocked::EmptySummary { segment: 0 })
    );
}

#[test]
fn a_summary_that_saves_nothing_is_refused() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Huge);
    match runtime().block_on(run(&pack, &settings(), &transport, &mut no_cancel())) {
        Err(CompactionBlocked::SummaryTooLarge {
            summary_tokens,
            allowance,
        }) => {
            assert_eq!(summary_tokens, 800);
            assert_eq!(allowance, 690, "700 effective minus 10 preserved");
        }
        other => panic!("expected a too-large refusal, got {other:?}"),
    }
}

#[test]
fn cancellation_before_the_first_call_makes_no_calls_at_all() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let (sender, mut receiver) = watch::channel(false);
    sender.send(true).expect("cancel");
    assert_eq!(
        runtime().block_on(run(&pack, &settings(), &transport, &mut receiver)),
        Err(CompactionBlocked::Cancelled)
    );
    assert!(transport.calls.lock().unwrap().is_empty());
}

#[test]
fn provider_usage_is_summed_and_absent_usage_stays_absent() {
    let mut settings = settings();
    settings.compaction_max_output_tokens = 350; // two segments
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(&pack, &settings, &transport, &mut no_cancel()))
        .expect("compaction runs");
    let usage = outcome.artifact.provider_usage.expect("usage reported");
    assert_eq!(usage.input_tokens, Some(240), "120 per segment x2");
    assert_eq!(usage.output_tokens, Some(22), "11 per segment x2");

    let transport = Fake::new(Behavior::NoUsage);
    let outcome = runtime()
        .block_on(run(&pack, &settings, &transport, &mut no_cancel()))
        .expect("compaction runs");
    assert_eq!(
        outcome.artifact.provider_usage, None,
        "unknown usage is never reported as zero"
    );
}

#[test]
fn the_artifact_records_the_revision_it_describes() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(&pack, &settings(), &transport, &mut no_cancel()))
        .expect("compaction runs");
    assert_eq!(
        outcome.artifact.observed_revision.as_deref(),
        Some("rev-abc"),
        "the summary says which workspace state it describes"
    );
}
