//! RUN-04 P1 evidence: read-only session open and deterministic replay.
//!
//! Shared fixtures for the recovery, resume and control suites live here
//! so all three drive the same durable shapes. Seeding always closes the
//! writer store explicitly before handing the database over, for the
//! reason documented in `history_tests.rs`: a dropped connection defers
//! the real close and can leave the WAL index lock held.

use crate::recovery::{
    ATTEMPT_KIND, AttemptRecord, BudgetRecord, MAX_RECORDS, OPERATION_KIND, OperationRecord,
    RecoveryError, RecoveryReader, SESSION_KIND, SessionRecord, journal_record_payload,
};
use crate::store::{DRIVE_DIRECTORY, EventRecord, StateUpdate, Store};
use crate::workspace::WriterOwnership;
use fluzo_core::application::{AttemptState, OperationState};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::{Builder, Runtime};

pub(crate) fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

pub(crate) struct Root(PathBuf);

impl Root {
    pub(crate) fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-recovery-{tag}-{}-{}",
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

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn drive(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn session_record(id: &str) -> SessionRecord {
    SessionRecord {
        session_id: id.to_owned(),
        workspace_fingerprint: "workspace-fingerprint-aaaa".to_owned(),
        git_branch: Some("main".to_owned()),
        git_revision: Some("1111111111111111111111111111111111111111".to_owned()),
        created_at_ms: 1_000,
        updated_at_ms: 2_000,
    }
}

pub(crate) fn attempt_record(
    attempt_id: &str,
    session_id: &str,
    state: AttemptState,
    ended_at_ms: Option<u64>,
) -> AttemptRecord {
    AttemptRecord {
        attempt_id: attempt_id.to_owned(),
        session_id: session_id.to_owned(),
        task_id: format!("task-of-{attempt_id}"),
        prior_attempt_id: None,
        state,
        started_at_ms: 3_000,
        ended_at_ms,
        budget: BudgetRecord::default(),
    }
}

pub(crate) fn operation_record(
    operation_id: &str,
    attempt_id: &str,
    session_id: &str,
    state: OperationState,
    effect_committed: bool,
    result_known: bool,
) -> OperationRecord {
    OperationRecord {
        operation_id: operation_id.to_owned(),
        attempt_id: attempt_id.to_owned(),
        session_id: session_id.to_owned(),
        subject: format!("file:src/{operation_id}.rs"),
        state,
        effect_committed,
        result_known,
        grant_id: Some(format!("grant-of-{operation_id}")),
    }
}

/// Writes one session with a completed attempt, an interrupted attempt,
/// a succeeded operation and one dispatched-with-no-result operation,
/// plus a journal whose ordinals skip 3 so a gap is present.
///
/// Returns after the writer store is closed, so the read-only opener
/// meets a released database.
pub(crate) fn seed_rich_session(rt: &Runtime, root: &Path, session_id: &str) {
    let mut store = rt
        .block_on(Store::open(root))
        .unwrap_or_else(|error| panic!("seed open failed: {error:?}"));
    let attempt_done = attempt_record(
        &format!("{session_id}-attempt-1"),
        session_id,
        AttemptState::Completed,
        Some(4_000),
    );
    let attempt_running = attempt_record(
        &format!("{session_id}-attempt-2"),
        session_id,
        AttemptState::Running,
        None,
    );
    let op_ok = operation_record(
        &format!("{session_id}-op-ok"),
        &attempt_done.attempt_id,
        session_id,
        OperationState::Succeeded,
        true,
        true,
    );
    let op_unknown = operation_record(
        &format!("{session_id}-op-unknown"),
        &attempt_running.attempt_id,
        session_id,
        OperationState::Dispatched,
        true,
        false,
    );
    let updates = vec![
        StateUpdate {
            id: session_id.to_owned(),
            kind: SESSION_KIND.to_owned(),
            payload: session_record(session_id).encode(),
        },
        StateUpdate {
            id: attempt_done.attempt_id.clone(),
            kind: ATTEMPT_KIND.to_owned(),
            payload: attempt_done.encode(),
        },
        StateUpdate {
            id: attempt_running.attempt_id.clone(),
            kind: ATTEMPT_KIND.to_owned(),
            payload: attempt_running.encode(),
        },
        StateUpdate {
            id: op_ok.operation_id.clone(),
            kind: OPERATION_KIND.to_owned(),
            payload: op_ok.encode(),
        },
        StateUpdate {
            id: op_unknown.operation_id.clone(),
            kind: OPERATION_KIND.to_owned(),
            payload: op_unknown.encode(),
        },
    ];
    // Ordinals 1, 2, 4: ordinal 3 is absent, which the replay must
    // surface as a gap rather than reconstruct or skip silently.
    let events = vec![
        EventRecord {
            state_id: session_id.to_owned(),
            kind: "turn".to_owned(),
            payload: journal_record_payload(1, 1_100, "user: fix the bug", None, None),
        },
        EventRecord {
            state_id: session_id.to_owned(),
            kind: "attempt".to_owned(),
            payload: journal_record_payload(
                2,
                1_200,
                "attempt started",
                Some(&attempt_done.attempt_id),
                None,
            ),
        },
        EventRecord {
            state_id: session_id.to_owned(),
            kind: "operation".to_owned(),
            payload: journal_record_payload(
                4,
                1_400,
                "operation dispatched",
                Some(&attempt_running.attempt_id),
                Some(&op_unknown.operation_id),
            ),
        },
    ];
    rt.block_on(store.commit_unit(&updates, &events)).unwrap();
    rt.block_on(store.close()).unwrap();
}

#[test]
fn open_read_only_refuses_any_write_at_the_driver() {
    let root = Root::new("write-refused");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-write");

    // The read-only guarantee is SQLite's, not this module's: a write
    // attempt fails at the driver rather than being declined by
    // convention. Tested on the connection itself so the assertion
    // cannot be satisfied by a method that merely chooses not to write.
    let mut store = rt
        .block_on(Store::open_read_only(root.path()))
        .expect("read-only open should succeed on a recorded store");
    let error = rt
        .block_on(store.commit_unit(
            &[StateUpdate {
                id: "s-write".into(),
                kind: SESSION_KIND.into(),
                payload: "{}".into(),
            }],
            &[],
        ))
        .expect_err("a read-only connection must refuse a write");
    assert_eq!(
        error,
        crate::store::StoreError::ReadOnly,
        "expected the SQLite read-only refusal (SQLITE_READONLY), not a collapsed class"
    );
    rt.block_on(store.close()).unwrap();
}

#[test]
fn open_never_creates_a_drive_or_a_database() {
    let root = Root::new("no-create");
    let rt = runtime();
    let error = rt
        .block_on(RecoveryReader::open(root.path()))
        .expect_err("opening an unrecorded workspace must not create one");
    assert!(
        matches!(
            error,
            RecoveryError::Store(crate::store::StoreError::NotFound)
        ),
        "expected NotFound, got {error:?}"
    );
    assert!(
        !root.drive().exists(),
        "a read-only open created the protected directory"
    );
}

#[test]
fn open_refuses_a_store_below_the_target_version() {
    let root = Root::new("old-version");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-old");
    // Downgrade the recorded version so the read-only open must refuse
    // rather than migrate: repairing a store belongs to the writer.
    rt.block_on(async {
        let mut store = Store::open(root.path()).await.unwrap();
        sqlx::query("UPDATE store_meta SET value = '1' WHERE key = 'schema_version'")
            .execute(store.connection_for_test())
            .await
            .unwrap();
        store.close().await.unwrap();
    });

    let error = rt
        .block_on(RecoveryReader::open(root.path()))
        .expect_err("a below-target store must not be silently migrated");
    assert!(
        matches!(
            error,
            RecoveryError::Store(crate::store::StoreError::MigrationRequired)
        ),
        "expected MigrationRequired, got {error:?}"
    );
}

#[test]
fn replay_is_deterministic_over_unchanged_records() {
    let root = Root::new("determinism");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-det");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let first = rt.block_on(reader.replay("s-det")).unwrap();
    let second = rt.block_on(reader.replay("s-det")).unwrap();
    assert!(
        !first.entries.is_empty(),
        "replay rendered nothing for a seeded session"
    );
    assert_eq!(
        first, second,
        "two replays of the same records differed; replay is not a pure reconstruction"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn replay_surfaces_a_missing_ordinal_as_a_gap() {
    let root = Root::new("gap");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-gap");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let replay = rt.block_on(reader.replay("s-gap")).unwrap();
    assert!(
        replay.gaps.iter().any(|gap| matches!(
            gap,
            crate::recovery::ReplayGap::MissingRecords {
                from: 3,
                to: 3,
                count: 1
            }
        )),
        "the absent ordinal 3 was not reported as a gap: {:?}",
        replay.gaps
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn running_attempt_without_terminal_record_is_interrupted_not_running() {
    let root = Root::new("interrupted");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-int");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-int")).unwrap();
    let running = session
        .attempts
        .iter()
        .find(|record| record.attempt_id.ends_with("attempt-2"))
        .expect("interrupted attempt not loaded");
    assert_eq!(running.state, AttemptState::Running);
    assert_eq!(
        running.effective_state(),
        AttemptState::Interrupted,
        "a record with no terminal outcome must not read as live or successful"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn dispatched_operation_without_result_is_unknown_not_repeatable() {
    let root = Root::new("unknown-op");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-unk");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let session = rt.block_on(reader.session("s-unk")).unwrap();
    let unknown = session
        .operations
        .iter()
        .find(|record| record.operation_id.ends_with("op-unknown"))
        .expect("unknown operation not loaded");
    assert!(
        unknown.effect_committed,
        "the fixture must model a side effect that may have happened"
    );
    assert!(!unknown.result_known);
    assert_eq!(
        unknown.effective_state(),
        OperationState::Unknown,
        "an operation that may have acted with no confirmed result must not read as Succeeded, Failed or repeatable"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn reading_does_not_take_the_writer_lock() {
    let root = Root::new("no-writer-lock");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-lock");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    rt.block_on(reader.list()).unwrap();
    // A reader must not become the owner: the writer lock is what makes
    // one runtime owner per workspace (A02), and an inspection client
    // stealing it would block the real owner.
    let ownership = WriterOwnership::acquire(root.path())
        .expect("a read-only reader must not hold the writer lock");
    assert_eq!(
        ownership.identity().fingerprint(),
        crate::workspace::WorkspaceIdentity::resolve(root.path())
            .unwrap()
            .fingerprint(),
    );
    drop(ownership);
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn list_is_bounded_rather_than_silently_truncated() {
    let root = Root::new("bounded");
    let rt = runtime();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    // One record past the read bound.
    let updates: Vec<StateUpdate> = (0..=MAX_RECORDS)
        .map(|index| StateUpdate {
            id: format!("session-{index:06}"),
            kind: SESSION_KIND.to_owned(),
            payload: session_record(&format!("session-{index:06}")).encode(),
        })
        .collect();
    rt.block_on(store.commit_unit(&updates, &[])).unwrap();
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let error = rt
        .block_on(reader.list())
        .expect_err("an over-bound listing must fail rather than truncate");
    assert!(
        matches!(error, RecoveryError::TooManyRecords { .. }),
        "expected TooManyRecords, got {error:?}"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn unknown_session_is_reported_not_rendered_empty() {
    let root = Root::new("unknown");
    let rt = runtime();
    seed_rich_session(&rt, root.path(), "s-known");

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let error = rt
        .block_on(reader.session("s-nope"))
        .expect_err("an unknown session id must not yield an empty success");
    assert!(
        matches!(error, RecoveryError::UnknownSession(_)),
        "expected UnknownSession, got {error:?}"
    );
    let replay_error = rt
        .block_on(reader.replay("s-nope"))
        .expect_err("replaying an unknown session must not silently render nothing");
    assert!(
        matches!(replay_error, RecoveryError::UnknownSession(_)),
        "expected UnknownSession from replay, got {replay_error:?}"
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn undecodable_record_is_surfaced_as_unknown_content() {
    let root = Root::new("undecodable");
    let rt = runtime();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    rt.block_on(store.commit_unit(
        &[StateUpdate {
            id: "s-bad".into(),
            kind: SESSION_KIND.into(),
            payload: session_record("s-bad").encode(),
        }],
        &[
            EventRecord {
                state_id: "s-bad".into(),
                kind: "turn".into(),
                payload: journal_record_payload(1, 1_000, "ok line", None, None),
            },
            EventRecord {
                state_id: "s-bad".into(),
                kind: "turn".into(),
                payload: "this is not JSON at all".into(),
            },
        ],
    ))
    .unwrap();
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let replay = rt.block_on(reader.replay("s-bad")).unwrap();
    assert_eq!(replay.entries.len(), 1, "the readable record was lost");
    assert!(
        replay
            .gaps
            .iter()
            .any(|gap| matches!(gap, crate::recovery::ReplayGap::Undecodable { .. })),
        "the unreadable record was not surfaced: {:?}",
        replay.gaps
    );
    rt.block_on(reader.close()).unwrap();
}

#[test]
fn encoded_records_decode_back_through_the_reader() {
    let root = Root::new("round-trip");
    let rt = runtime();
    let mut store = rt.block_on(Store::open(root.path())).unwrap();
    let session = session_record("s-rt");
    let attempt = AttemptRecord {
        attempt_id: "s-rt-attempt-9".to_owned(),
        session_id: "s-rt".to_owned(),
        task_id: "task-9".to_owned(),
        prior_attempt_id: Some("s-rt-attempt-8".to_owned()),
        state: AttemptState::Interrupted,
        started_at_ms: 7,
        ended_at_ms: Some(9),
        budget: BudgetRecord {
            tokens: 120,
            active_seconds: 33,
            tool_calls: 5,
            turns: 4,
            uncertain: true,
        },
    };
    let operation = OperationRecord {
        operation_id: "s-rt-op-9".to_owned(),
        attempt_id: attempt.attempt_id.clone(),
        session_id: "s-rt".to_owned(),
        subject: "command:/bin/true".to_owned(),
        state: OperationState::AwaitingApproval,
        effect_committed: false,
        result_known: false,
        grant_id: None,
    };
    rt.block_on(store.commit_unit(
        &[
            StateUpdate {
                id: "s-rt".into(),
                kind: SESSION_KIND.into(),
                payload: session.encode(),
            },
            StateUpdate {
                id: attempt.attempt_id.clone(),
                kind: ATTEMPT_KIND.into(),
                payload: attempt.encode(),
            },
            StateUpdate {
                id: operation.operation_id.clone(),
                kind: OPERATION_KIND.into(),
                payload: operation.encode(),
            },
        ],
        &[],
    ))
    .unwrap();
    rt.block_on(store.close()).unwrap();

    let mut reader = rt.block_on(RecoveryReader::open(root.path())).unwrap();
    let loaded = rt.block_on(reader.session("s-rt")).unwrap();
    assert_eq!(loaded.session, session);
    assert_eq!(loaded.attempts, vec![attempt]);
    assert_eq!(loaded.operations, vec![operation]);
    rt.block_on(reader.close()).unwrap();
}
