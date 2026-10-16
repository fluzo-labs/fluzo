//! AGT-02 P3: compaction persistence and reversibility tests.
//!
//! Compaction is only trustworthy if the artifact lands whole, can be read
//! back with its audit trail intact, and can be undone exactly. These run
//! against the real SQLite journal.

use fluzo_core::context::{Retention, SourceKind, TrustTier};

use crate::compaction::*;
use crate::compaction_tests::{Behavior, Fake, no_cancel, pack_with, runtime};
use crate::store::Store;

fn fresh_root(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fluzo-compaction-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).expect("root");
    path
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_dir_all(path);
}

// ---------------------------------------------------------------------------
// Reversibility
// ---------------------------------------------------------------------------

#[test]
fn reversal_restores_exactly_what_the_summary_replaced() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("compaction runs");
    let plan = plan(&pack, &crate::compaction_tests::settings()).expect("plan");

    let restored = reverse(&outcome.items, &plan.summarized, &outcome.artifact);
    let mut restored_ids: Vec<&str> = restored
        .iter()
        .map(|item| item.source.identity.as_str())
        .collect();
    restored_ids.sort();

    let mut expected: Vec<&str> = Vec::new();
    expected.push("workspace:revision");
    expected.push("task:t1");
    expected.push("AGENTS.md");
    let turns: Vec<String> = (0..6).map(|i| format!("turn:{i}")).collect();
    let turn_refs: Vec<&str> = turns.iter().map(String::as_str).collect();
    expected.extend(turn_refs);
    expected.sort();

    assert_eq!(restored_ids, expected, "reversal is exact, not approximate");
    assert!(
        !restored
            .iter()
            .any(|item| item.source.kind == SourceKind::CompactionSummary),
        "the summary is gone once reversed"
    );
}

#[test]
fn reversal_leaves_items_the_summary_never_covered_alone() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("compaction runs");
    // Only one turn was ever covered; the others must not be re-added.
    let one = pack
        .included
        .iter()
        .filter(|item| item.source.identity == "turn:0")
        .cloned()
        .collect::<Vec<_>>();
    let mut artifact = outcome.artifact.clone();
    artifact.covers = vec![one[0].source.clone()];

    let restored = reverse(&outcome.items, &pack.included, &artifact);
    let ids: Vec<&str> = restored
        .iter()
        .map(|i| i.source.identity.as_str())
        .collect();
    assert!(ids.contains(&"turn:0"));
    assert!(
        !ids.contains(&"turn:1"),
        "uncovered items are not resurrected"
    );
}

// ---------------------------------------------------------------------------
// Durable commit
// ---------------------------------------------------------------------------

#[test]
fn a_committed_artifact_lands_as_one_state_row_and_one_journal_event() {
    let root = fresh_root("commit");
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("compaction runs");

    runtime().block_on(async {
        let mut store = Store::open(&root).await.expect("store opens");
        commit(&mut store, "s-1", &outcome.artifact)
            .await
            .expect("commit lands");

        let payload = committed_payload(&mut store, "s-1")
            .await
            .expect("artifact readable back");
        assert!(payload.contains("\"segments\":1"), "{payload}");
        assert!(payload.contains("\"replaced_tokens\":600"), "{payload}");
        assert!(
            payload.contains("\"summary_tokens\":5"),
            "the summary's own cost is recorded: {payload}"
        );
        assert!(payload.contains("rev-abc"), "{payload}");
        assert!(
            payload.contains("turn:0") && payload.contains("turn:5"),
            "the payload names what the summary covers: {payload}"
        );
        assert!(
            payload.contains("\"input_tokens\":120"),
            "provider-reported usage is part of the audit record: {payload}"
        );

        let events = store.events_after(0, 100).await.expect("events read");
        let compaction_events: Vec<_> = events
            .iter()
            .filter(|event| event.kind == COMPACTION_EVENT_KIND)
            .collect();
        assert_eq!(compaction_events.len(), 1, "exactly one compaction event");
        assert_eq!(compaction_events[0].state_id, "compaction:s-1");

        store.close().await.expect("store closes");
    });
    cleanup(&root);
}

#[test]
fn a_second_compaction_updates_the_same_row_rather_than_forking_state() {
    let root = fresh_root("recommit");
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let first = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("first compaction");
    let second = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("second compaction");

    runtime().block_on(async {
        let mut store = Store::open(&root).await.expect("store opens");
        commit(&mut store, "s-1", &first.artifact)
            .await
            .expect("first commit");
        commit(&mut store, "s-1", &second.artifact)
            .await
            .expect("second commit");

        let (kind, _payload, revision) = store
            .state("compaction:s-1")
            .await
            .expect("state present")
            .expect("row exists");
        assert_eq!(kind, COMPACTION_STATE_KIND);
        assert_eq!(revision, 2, "the same row was updated, not duplicated");
        store.close().await.expect("store closes");
    });
    cleanup(&root);
}

#[test]
fn a_failed_commit_leaves_no_artifact_behind() {
    let root = fresh_root("failed-commit");
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("compaction runs");

    runtime().block_on(async {
        // Seed the store so a read-only reopen is valid, then close it.
        {
            let mut writer = Store::open(&root).await.expect("writer opens");
            writer.commit_unit(&[], &[]).await.ok();
            writer.close().await.expect("writer closes");
        }

        let mut read_only = Store::open_read_only(&root).await.expect("read-only opens");
        let error = commit(&mut read_only, "s-1", &outcome.artifact)
            .await
            .expect_err("a read-only journal cannot accept a compaction");
        assert!(
            matches!(error, CompactionBlocked::Store(_)),
            "reported as a store failure: {error:?}"
        );
        assert_eq!(
            committed_payload(&mut read_only, "s-1").await,
            None,
            "nothing landed: no half-written compaction is visible"
        );
    });
    cleanup(&root);
}

#[test]
fn the_summary_item_is_never_authoritative_however_it_is_read() {
    let pack = pack_with(6);
    let transport = Fake::new(Behavior::Summarize);
    let outcome = runtime()
        .block_on(run(
            &pack,
            &crate::compaction_tests::settings(),
            &transport,
            &mut no_cancel(),
        ))
        .expect("compaction runs");
    let summary = outcome.artifact.as_context_item();
    assert_eq!(summary.tier, TrustTier::ModelText);
    assert_eq!(summary.retention, Retention::Optional);
    assert!(
        summary.source.identity.starts_with("compaction:"),
        "the summary identifies itself as one: {}",
        summary.source.identity
    );
    assert!(
        !outcome
            .items
            .iter()
            .any(|item| item.source.kind == SourceKind::CompactionSummary
                && item.tier.can_authorize()),
        "no compaction summary can carry a grant"
    );
}
