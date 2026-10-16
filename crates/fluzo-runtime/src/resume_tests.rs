//! RUN-04 P2 evidence: resume reconciliation.
//!
//! DAT-style deterministic crash boundaries: seed a durable state that
//! models a specific crash point, reconcile it read-only, apply it, then
//! read the result back. No sleeps, no mocks of the store, no live
//! inference.

use crate::recovery::{
    ATTEMPT_KIND, AttemptRecord, BudgetRecord, OPERATION_KIND, OperationRecord, RecoveryReader,
    SESSION_KIND,
};
use crate::resume::{
    self, LiveWorkspace, ReconcileBlock, ResumeError, resumed_authorization,
};
use crate::session_auth::ApprovalMode;
use crate::store::{EventRecord, StateUpdate, Store};
use crate::workspace::WorkspaceIdentity;
use fluzo_core::application::{AttemptState, OperationState};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(500);

struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-resume-{tag}-{}-{}",
            process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const RECORDED_REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DRIFTED_REVISION: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Seeds a session whose second attempt was still running when the writer
/// stopped, with one succeeded operation, one dispatched-without-result
/// operation carrying a grant, and one awaiting-approval operation.
fn seed_crashed_session(rt: &Runtime, root: &Path, session_id: &str) {
    let mut store = rt
        .block_on(Store::open(root))
        .unwrap_or_else(|error| panic!("seed open failed: {error:?}"));
    let completed = AttemptRecord {
        attempt_id: format!("{session_id}-attempt-0001"),
        session_id: session_id.to_owned(),
        task_id: "task-1".to_owned(),
        prior_attempt_id: None,
        state: AttemptState::Completed,
        started_at_ms: 1_000,
        ended_at_ms: Some(2_000),
        budget: BudgetRecord {
            tokens: 900,
            active_seconds: 30,
            tool_calls: 4,
            turns: 3,
            uncertain: false,
        },
    };
    let running = AttemptRecord {
        attempt_id: format!("{session_id}-attempt-0002"),
        session_id: session_id.to_owned(),
        task_id: "task-1".to_owned(),
        prior_attempt_id: Some(completed.attempt_id.clone()),
        state: AttemptState::Running,
        started_at_ms: 3_000,
        ended_at_ms: None,
        budget: BudgetRecord {
            tokens: 400,
            active_seconds: 12,
            tool_calls: 2,
            turns: 1,
            uncertain: false,
        },
    };
    let succeeded = OperationRecord {
        operation_id: format!("{session_id}-op-succeeded"),
        attempt_id: completed.attempt_id.clone(),
        session_id: session_id.to_owned(),
        subject: "file:src/lib.rs".to_owned(),
        state: OperationState::Succeeded,
        effect_committed: true,
        result_known: true,
        grant_id: Some(format!("{session_id}-grant-succeeded")),
    };
    // Crash after the side effect, before the result commit.
    let after_effect = OperationRecord {
        operation_id: format!("{session_id}-op-after-effect"),
        attempt_id: running.attempt_id.clone(),
        session_id: session_id.to_owned(),
        subject: "file:src/main.rs".to_owned(),
        state: OperationState::Dispatched,
        effect_committed: true,
        result_known: false,
        grant_id: Some(format!("{session_id}-grant-after-effect")),
    };
    // Crash after intent commit, before dispatch.
    let never_dispatched = OperationRecord {
        operation_id: format!("{session_id}-op-awaiting"),
        attempt_id: running.attempt_id.clone(),
        session_id: session_id.to_owned(),
        subject: "command:/bin/true".to_owned(),
        state: OperationState::AwaitingApproval,
        effect_committed: false,
        result_known: false,
        grant_id: Some(format!("{session_id}-grant-awaiting")),
    };
    let updates = vec![
        StateUpdate {
            id: session_id.to_owned(),
            kind: SESSION_KIND.to_owned(),
            payload: session_payload(session_id),
        },
        StateUpdate {
            id: completed.attempt_id.clone(),
            kind: ATTEMPT_KIND.to_owned(),
            payload: completed.encode(),
        },
        StateUpdate {
            id: running.attempt_id.clone(),
            kind: ATTEMPT_KIND.to_owned(),
            payload: running.encode(),
        },
        StateUpdate {
            id: succeeded.operation_id.clone(),
            kind: OPERATION_KIND.to_owned(),
            payload: succeeded.encode(),
        },
        StateUpdate {
            id: after_effect.operation_id.clone(),
            kind: OPERATION_KIND.to_owned(),
            payload: after_effect.encode(),
        },
        StateUpdate {
            id: never_dispatched.operation_id.clone(),
            kind: OPERATION_KIND.to_owned(),
            payload: never_dispatched.encode(),
        },
    ];
    let events = vec![EventRecord {
        state_id: session_id.to_owned(),
        kind: "turn".to_owned(),
        payload: crate::recovery::journal_record_payload(1, 1_100, "user: change", None, None),
    }];
    rt.block_on(store.commit_unit(&updates, &events)).unwrap();
    rt.block_on(store.close()).unwrap();
}

fn session_payload(session_id: &str) -> String {
    format!(
        "{{\"v\":1,\"session_id\":\"{session_id}\",\"workspace_fingerprint\":\"FP\",\"git_branch\":\"main\",\"git_revision\":\"{RECORDED_REVISION}\",\"created_at_ms\":1000,\"updated_at_ms\":2000}}"
    )
}

/// Replaces the recorded workspace fingerprint so a mismatch can be
/// tested against the real identity of the temp root.
fn record_fingerprint(rt: &Runtime, root: &Path, session_id: &str) {
    let fingerprint = WorkspaceIdentity::resolve(root).unwrap().fingerprint();
    let mut store = rt.block_on(Store::open(root)).unwrap();
    rt.block_on(store.commit_unit(
        &[StateUpdate {
            id: session_id.to_owned(),
            kind: SESSION_KIND.to_owned(),
            payload: format!(
                "{{\"v\":1,\"session_id\":\"{session_id}\",\"workspace_fingerprint\":\"{fingerprint}\",\"git_branch\":\"main\",\"git_revision\":\"{RECORDED_REVISION}\",\"created_at_ms\":1000,\"updated_at_ms\":2000}}"
            ),
        }],
        &[],
    ))
    .unwrap();
    rt.block_on(store.close()).unwrap();
}

async fn reconcile_with(
    root: &Path,
    session_id: &str,
    revision: &str,
) -> Result<resume::Reconciliation, ReconcileBlock> {
    let mut reader = RecoveryReader::open(root).await.unwrap();
    let live = LiveWorkspace {
        fingerprint: WorkspaceIdentity::resolve(root).unwrap().fingerprint(),
        git_revision: Some(revision.to_owned()),
    };
    let result = resume::reconcile(&mut reader, session_id, &live).await;
    let _ = reader.close().await;
    result
}

#[test]
fn reconcile_itself_writes_nothing() {
    let root = Root::new("reconcile-readonly");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-rr");
    record_fingerprint(&rt, root.path(), "s-rr");

    let before = count_rows(&rt, root.path());
    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-rr", RECORDED_REVISION))
        .expect("reconcile should succeed on a matching workspace");
    let after = count_rows(&rt, root.path());
    assert_eq!(
        before, after,
        "reconcile mutated the store; it must stay read-only until applied"
    );
    assert_eq!(reconciliation.interrupted_attempts.len(), 1);
    assert_eq!(reconciliation.unknown_operations.len(), 1);
    assert_eq!(reconciliation.dropped_pending_approvals.len(), 1);
    assert!(
        reconciliation.stale_claims.is_empty(),
        "no revision drift was present, so nothing should be stale"
    );
}

#[test]
fn resume_without_confirmation_writes_nothing() {
    let root = Root::new("no-confirm");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-nc");
    record_fingerprint(&rt, root.path(), "s-nc");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-nc", RECORDED_REVISION))
        .unwrap();
    let before = count_rows(&rt, root.path());
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    let error = rt
        .block_on(resume::apply(&mut store, &reconciliation, false))
        .expect_err("an unconfirmed resume must not proceed");
    assert_eq!(error, ResumeError::ConfirmationRequired);
    rt.block_on(store.close()).unwrap();
    let after = count_rows(&rt, root.path());
    assert_eq!(before, after, "an unconfirmed resume changed the store");
}

#[test]
fn workspace_mismatch_blocks_and_writes_nothing() {
    let root = Root::new("mismatch");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-mm");
    // Leave the recorded fingerprint as the literal "FP", which cannot
    // equal the real identity of the temp root.
    let before = count_rows(&rt, root.path());
    let error = rt
        .block_on(reconcile_with(root.path(), "s-mm", RECORDED_REVISION))
        .expect_err("a mismatched workspace must block the resume");
    assert!(
        matches!(error, ReconcileBlock::WorkspaceMismatch { .. }),
        "expected WorkspaceMismatch, got {error:?}"
    );
    let after = count_rows(&rt, root.path());
    assert_eq!(before, after);
    if let ReconcileBlock::WorkspaceMismatch { recorded, observed } = error {
        assert_eq!(recorded, "FP");
        assert_ne!(observed, "FP");
    }
}

#[test]
fn apply_interrupts_the_running_attempt_and_links_a_new_one() {
    let root = Root::new("apply");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-ap");
    record_fingerprint(&rt, root.path(), "s-ap");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-ap", RECORDED_REVISION))
        .unwrap();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    let applied = rt
        .block_on(resume::apply(&mut store, &reconciliation, true))
        .expect("confirmed resume should apply");
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-ap")).unwrap();
    assert_eq!(
        session.attempts.len(),
        3,
        "exactly one new attempt must be created"
    );
    let interrupted = session
        .attempts
        .iter()
        .find(|record| record.attempt_id.ends_with("attempt-0002"))
        .unwrap();
    assert_eq!(
        interrupted.state,
        AttemptState::Interrupted,
        "the running attempt must be durably interrupted, not left running"
    );
    assert!(interrupted.ended_at_ms.is_some());
    let new_attempt = session
        .attempts
        .iter()
        .find(|record| record.attempt_id == applied.attempt_id)
        .expect("the applied attempt must be readable");
    assert_eq!(new_attempt.state, AttemptState::Pending);
    assert_eq!(
        new_attempt.prior_attempt_id.as_deref(),
        Some(format!("{}-attempt-0002", "s-ap").as_str()),
        "the new attempt must be linked to the attempt it follows"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn unknown_operation_stays_unknown_and_is_never_replayed() {
    let root = Root::new("no-replay");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-nr");
    record_fingerprint(&rt, root.path(), "s-nr");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-nr", RECORDED_REVISION))
        .unwrap();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    rt.block_on(resume::apply(&mut store, &reconciliation, true))
        .unwrap();
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-nr")).unwrap();
    let unknown = session
        .operations
        .iter()
        .find(|record| record.operation_id.ends_with("op-after-effect"))
        .unwrap();
    assert_eq!(unknown.state, OperationState::Unknown);
    assert!(
        unknown.effect_committed && !unknown.result_known,
        "the durable record must still say a side effect may have happened with no confirmed result"
    );
    assert_eq!(
        session
            .operations
            .iter()
            .filter(|record| record.subject.ends_with("src/main.rs"))
            .count(),
        1,
        "resume must not create a second operation for the same subject; no automatic replay"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn known_budget_carries_forward_exactly() {
    let root = Root::new("budget");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-bf");
    record_fingerprint(&rt, root.path(), "s-bf");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-bf", RECORDED_REVISION))
        .unwrap();
    assert_eq!(reconciliation.carry_forward.tokens, 1_300);
    assert_eq!(reconciliation.carry_forward.active_seconds, 42);
    assert_eq!(reconciliation.carry_forward.tool_calls, 6);
    assert_eq!(reconciliation.carry_forward.turns, 4);
    assert!(
        !reconciliation.carry_forward.uncertain,
        "every seeded contribution was certain"
    );

    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    rt.block_on(resume::apply(&mut store, &reconciliation, true))
        .unwrap();
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-bf")).unwrap();
    let new_attempt = session
        .attempts
        .iter()
        .find(|record| record.attempt_id == reconciliation.new_attempt_id)
        .unwrap();
    assert_eq!(
        new_attempt.budget.tokens, 1_300,
        "the resumed attempt must start from the consumed total, not from zero"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn uncertain_budget_is_carried_as_uncertain() {
    let root = Root::new("uncertain-budget");
    let rt = runtime();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    rt.block_on(store.commit_unit(
        &[
            StateUpdate {
                id: "s-ub".into(),
                kind: SESSION_KIND.into(),
                payload: session_payload("s-ub"),
            },
            StateUpdate {
                id: "s-ub-attempt-0001".into(),
                kind: ATTEMPT_KIND.into(),
                payload: AttemptRecord {
                    attempt_id: "s-ub-attempt-0001".into(),
                    session_id: "s-ub".into(),
                    task_id: "t".into(),
                    prior_attempt_id: None,
                    state: AttemptState::Running,
                    started_at_ms: 1,
                    ended_at_ms: None,
                    budget: BudgetRecord {
                        tokens: 700,
                        active_seconds: 9,
                        tool_calls: 1,
                        turns: 1,
                        uncertain: true,
                    },
                }
                .encode(),
            },
        ],
        &[],
    ))
    .unwrap();
    rt.block_on(store.close()).unwrap();
    record_fingerprint(&rt, root.path(), "s-ub");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-ub", RECORDED_REVISION))
        .unwrap();
    assert!(
        reconciliation.carry_forward.uncertain,
        "an unconfirmed contribution must make the whole carry-forward uncertain, so it cannot be read as a trustworthy remaining-budget basis"
    );
}

#[test]
fn interrupted_attempt_grants_do_not_carry_over() {
    let root = Root::new("grants");
    let rt: Runtime = runtime();
    seed_crashed_session(&rt, root.path(), "s-gr");
    record_fingerprint(&rt, root.path(), "s-gr");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-gr", RECORDED_REVISION))
        .unwrap();
    assert_eq!(
        reconciliation.expired_grants.len(),
        2,
        "the two grants held by operations that never reached a terminal state must be expired"
    );
    assert!(
        !reconciliation
            .expired_grants
            .iter()
            .any(|grant| grant.ends_with("grant-succeeded")),
        "a grant whose operation completed is not part of the interrupted attempt's expired set"
    );
    // The resumed attempt carries no grant reference of its own.
    assert_eq!(
        reconciliation.new_attempt_id,
        "s-gr-attempt-0003",
        "the new attempt must be the deterministic successor"
    );
}

#[test]
fn yolo_is_never_restored_by_resume() {
    // RUN-02 keeps YOLO in process memory with no settings key, so a
    // resumed process has nothing to restore. The only construction
    // path resume offers starts with it off.
    let service = resumed_authorization(ApprovalMode::Interactive);
    assert!(!service.yolo_enabled());
    let headless = resumed_authorization(ApprovalMode::Headless);
    assert!(!headless.yolo_enabled());
}

#[test]
fn revision_drift_invalidates_recorded_claims() {
    let root = Root::new("drift");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-dr");
    record_fingerprint(&rt, root.path(), "s-dr");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-dr", DRIFTED_REVISION))
        .expect("drift does not block the resume; it invalidates claims");
    assert!(
        !reconciliation.stale_claims.is_empty(),
        "a moved revision must invalidate the recorded verification and approval claims"
    );
    for claim in &reconciliation.stale_claims {
        match claim {
            resume::StaleClaim::VerificationRevision { recorded, observed, .. }
            | resume::StaleClaim::EditApproval { recorded, observed, .. } => {
                assert_eq!(recorded, RECORDED_REVISION);
                assert_eq!(observed, DRIFTED_REVISION);
            }
        }
    }
    assert!(
        reconciliation
            .stale_claims
            .iter()
            .any(|claim| matches!(claim, resume::StaleClaim::VerificationRevision { .. })),
        "the succeeded operation's verification must be surfaced as stale"
    );
}

#[test]
fn applying_twice_reports_the_existing_attempt_instead_of_duplicating() {
    let root = Root::new("idempotent");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-id");
    record_fingerprint(&rt, root.path(), "s-id");

    let reconciliation = rt
        .block_on(reconcile_with(root.path(), "s-id", RECORDED_REVISION))
        .unwrap();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    rt.block_on(resume::apply(&mut store, &reconciliation, true))
        .unwrap();
    let error = rt
        .block_on(resume::apply(&mut store, &reconciliation, true))
        .expect_err("a retried resume must not create a second attempt");
    assert!(
        matches!(error, ResumeError::AlreadyApplied { .. }),
        "expected AlreadyApplied, got {error:?}"
    );
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-id")).unwrap();
    assert_eq!(session.attempts.len(), 3, "exactly one resume attempt exists");
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn unknown_session_blocks_rather_than_resuming_an_empty_shape() {
    let root = Root::new("unknown-session");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-real");
    let error = rt
        .block_on(reconcile_with(root.path(), "s-ghost", RECORDED_REVISION))
        .expect_err("an unknown session must not reconcile into an empty resume");
    assert!(
        matches!(error, ReconcileBlock::SessionUnavailable(_)),
        "expected SessionUnavailable, got {error:?}"
    );
}

#[test]
fn missing_live_revision_blocks_the_resume() {
    let root = Root::new("no-revision");
    let rt = runtime();
    seed_crashed_session(&rt, root.path(), "s-nrev");
    record_fingerprint(&rt, root.path(), "s-nrev");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let live = LiveWorkspace {
        fingerprint: WorkspaceIdentity::resolve(root.path()).unwrap().fingerprint(),
        git_revision: None,
    };
    let error = rt
        .block_on(resume::reconcile(&mut reader, "s-nrev", &live))
        .expect_err("an unknown live revision cannot validate recorded claims");
    assert!(
        matches!(error, ReconcileBlock::RevisionUnknown(_)),
        "expected RevisionUnknown, got {error:?}"
    );
    rt.block_on(reader.close()).unwrap();
}

/// Counts state rows so a test can assert "wrote nothing" rather than
/// trusting that no method was called.
fn count_rows(rt: &Runtime, root: &Path) -> usize {
    let mut store = rt.block_on(Store::open(root)).unwrap();
    let mut total = 0usize;
    for kind in [SESSION_KIND, ATTEMPT_KIND, OPERATION_KIND] {
        total += rt
            .block_on(store.states_of_kind(kind, 10_000))
            .unwrap()
            .len();
    }
    rt.block_on(store.close()).unwrap();
    total
}
