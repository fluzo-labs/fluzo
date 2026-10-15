//! Regression coverage for the `history` facade: the store is opened
//! once per command and released deterministically.
//!
//! This exists because the CLI originally opened the store once per step
//! (listing, plan, delete) and dropped each connection instead of
//! closing it. Dropping defers the real close to a background thread,
//! so the next open can meet a still-held WAL index lock and fail with
//! SQLITE_BUSY under `busy_timeout = 0`. The failure showed up only on
//! CI hardware, which is exactly why the contract is asserted here
//! rather than left to timing.

use crate::artifact::{ArtifactSpec, CaptureRequest};
use crate::history::{self, History, HistoryError};
use crate::purge::Confirmation;
use crate::store::{DRIVE_DIRECTORY, EventRecord, StateUpdate, Store};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-history-{tag}-{}-{}",
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
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn seed(rt: &Runtime, root: &Path, id: &str, content: &[u8]) {
    let mut store = rt
        .block_on(Store::open(root))
        .unwrap_or_else(|error| panic!("seed open failed: {error:?}"));
    rt.block_on(store.commit_unit(
        &[StateUpdate {
            id: id.into(),
            kind: "session".into(),
            payload: "created".into(),
        }],
        &[EventRecord {
            state_id: id.into(),
            kind: "turn".into(),
            payload: "first".into(),
        }],
    ))
    .unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&CaptureRequest {
            spec: ArtifactSpec {
                id: format!("{id}-art"),
                state_id: id.into(),
                kind: "diff".into(),
                provenance: "fixture".into(),
            },
            content: content.to_vec(),
            secrets: Vec::new(),
            omitted: false,
        }))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&record, &[], &[]))
        .unwrap();
    // Explicit close, not a drop: the seed store hands the database to a
    // later opener and must not leave the WAL index held.
    rt.block_on(store.close()).unwrap();
}

/// The exact per-command pattern the CLI now uses: one open, several
/// steps, one deterministic close, repeated. Any stage that returns
/// `Busy` fails here with its stage named.
#[test]
fn facade_command_cycles_never_meet_a_busy_store() {
    let root = Root::new("cycle");
    let rt = runtime();
    for cycle in 0..25 {
        seed(&rt, &root.0, &format!("session-{cycle}"), b"payload bytes");

        let mut history = History::open(&root.0)
            .unwrap_or_else(|error| panic!("cycle {cycle}: open failed: {error}"));
        let sessions = history
            .list()
            .unwrap_or_else(|error| panic!("cycle {cycle}: list failed: {error}"));
        assert!(
            !sessions.is_empty(),
            "cycle {cycle}: listing came back empty after seeding"
        );
        let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
        let plan = history
            .plan(&ids)
            .unwrap_or_else(|error| panic!("cycle {cycle}: plan failed: {error}"));
        assert!(
            plan.total_bytes_released > 0,
            "cycle {cycle}: plan released nothing",
        );
        let outcome = history
            .purge(&ids, Confirmation::ExplicitYes)
            .unwrap_or_else(|error| panic!("cycle {cycle}: purge failed: {error}"));
        assert!(
            outcome.files_pending.is_empty(),
            "cycle {cycle}: files left pending: {:?}",
            outcome.files_pending
        );
        history
            .close()
            .unwrap_or_else(|error| panic!("cycle {cycle}: close failed: {error}"));
    }
}

/// A dropped store must not be what the next opener finds. This is the
/// narrower form of the regression: open, read, close, then open again
/// and take the write lock.
#[test]
fn explicit_close_lets_the_next_opener_write_immediately() {
    let root = Root::new("close-then-write");
    let rt = runtime();
    seed(&rt, &root.0, "alpha", b"shared payload");
    for attempt in 0..40 {
        let mut history = History::open(&root.0).expect("open");
        let _ = history.list().expect("list");
        history.close().expect("close");

        let mut writer = rt
            .block_on(Store::open(&root.0))
            .unwrap_or_else(|error| panic!("attempt {attempt}: reopen failed: {error:?}"));
        rt.block_on(writer.attach_session("alpha", attempt))
            .unwrap_or_else(|error| panic!("attempt {attempt}: write failed: {error:?}"));
        rt.block_on(writer.detach_session("alpha")).unwrap();
        rt.block_on(writer.close()).unwrap();
    }
}

/// A store failure must name the stage that produced it. A bare `Busy`
/// cannot be diagnosed after the fact.
#[test]
fn store_failures_name_the_failing_stage() {
    let missing = std::env::temp_dir().join(format!(
        "fluzo-history-missing-{}-{}",
        process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let error = History::open(&missing)
        .err()
        .expect("opening a missing path must fail");
    match &error {
        HistoryError::Store { stage, .. } => assert_eq!(*stage, "open"),
        other => panic!("expected a staged store failure, got {other:?}"),
    }
    assert!(
        error.to_string().contains("at open"),
        "message must carry the stage: {error}"
    );
    assert_eq!(error.code(), "storage_unavailable");
    let _ = fs::remove_dir_all(missing);
}

/// Listing must not create a drive; the facade reports absence instead.
#[test]
fn drive_absence_is_reported_not_created() {
    let root = Root::new("no-drive");
    assert!(!history::drive_present(&root.0));
    assert!(!root.0.join(DRIVE_DIRECTORY).exists());
}
