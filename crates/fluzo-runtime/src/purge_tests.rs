//! DAT-02d history purge: dry-run purity, shared-reference protection,
//! unbypassable confirmation, active-session protection, idempotent
//! retry and quota release.
//!
//! Fixtures follow the hardened #81 patterns: canonicalized temp roots so
//! a symlinked TMPDIR cannot change the fixture's identity, explicit
//! 0700 so the runner umask cannot widen them, and `expect_open` which
//! retries only environment-flake classes and logs every failed attempt.
//! Bare `unwrap()` on an open or reopen is deliberately not used.

use crate::artifact::{ARTIFACTS_DIRECTORY, ArtifactSpec, CaptureRequest};
use crate::purge::*;
use crate::quota::{FixedFreeSpace, QuotaClass, QuotaLimits};
use crate::store::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
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
            "fluzo-purge-{tag}-{}-{}",
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

    fn drive(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY)
    }

    fn artifacts(&self) -> PathBuf {
        self.drive().join(ARTIFACTS_DIRECTORY)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Opens with the quota guard attached and a fixed free-space reading, so
/// no test depends on the real disk. Retries only the environment-flake
/// classes and reports every failed attempt, per the #81 discipline.
fn expect_open(rt: &Runtime, root: &Root, context: &str) -> Store {
    let max_attempts = 4;
    let mut delay = Duration::from_millis(50);
    for attempt in 1..=max_attempts {
        match rt.block_on(Store::open_quotated_with_probe(
            &root.0,
            QuotaLimits::default(),
            FixedFreeSpace(1 << 40),
        )) {
            Ok(store) => return store,
            Err(error)
                if attempt < max_attempts
                    && matches!(
                        error,
                        StoreError::Inaccessible | StoreError::Io | StoreError::Busy
                    ) =>
            {
                crate::test_diagnostics::report_and_log(
                    &root.0,
                    &format!("{context} (attempt {attempt}, retrying)"),
                    error,
                );
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => {
                crate::test_diagnostics::report_and_log(&root.0, context, error);
                panic!("open refused: {error:?}");
            }
        }
    }
    unreachable!()
}

fn update(id: &str, payload: &str) -> StateUpdate {
    StateUpdate {
        id: id.into(),
        kind: "session".into(),
        payload: payload.into(),
    }
}

fn event(state_id: &str, payload: &str) -> EventRecord {
    EventRecord {
        state_id: state_id.into(),
        kind: "turn".into(),
        payload: payload.into(),
    }
}

fn capture(id: &str, state_id: &str, content: &[u8]) -> CaptureRequest {
    CaptureRequest {
        spec: ArtifactSpec {
            id: id.into(),
            state_id: state_id.into(),
            kind: "diff".into(),
            provenance: "fixture".into(),
        },
        content: content.to_vec(),
        secrets: Vec::new(),
        omitted: false,
    }
}

/// Creates a session with one event and one artifact of `content`.
fn seed_session(rt: &Runtime, store: &mut Store, id: &str, content: &[u8]) {
    rt.block_on(store.commit_unit(&[update(id, "created")], &[event(id, "first")]))
        .unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&capture(&format!("{id}-art"), id, content)))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&record, &[], &[]))
        .unwrap();
}

/// A content-addressed fingerprint of everything the purge could change:
/// the main database file and every artifact file's name and bytes. The
/// `-shm` file is excluded because it is shared memory and may move
/// without any logical change; the WAL is folded into the main file first
/// so the fingerprint compares logical state rather than journal layout.
fn fingerprint(rt: &Runtime, store: &mut Store, root: &Root) -> String {
    rt.block_on(store.checkpoint_for_test()).unwrap();
    let mut hasher = Sha256::new();
    let db = root.drive().join(DATABASE_FILE);
    hasher.update(fs::read(&db).unwrap_or_default());
    hasher.update(b"|artifacts|");
    let mut entries: Vec<String> = fs::read_dir(root.artifacts())
        .map(|dir| {
            dir.filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    for name in entries {
        hasher.update(name.as_bytes());
        hasher.update(b":");
        hasher.update(fs::read(root.artifacts().join(&name)).unwrap_or_default());
        hasher.update(b";");
    }
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn artifact_exists(root: &Root, id: &str) -> bool {
    root.artifacts().join(id).exists()
}

fn artifacts_bytes(root: &Root) -> u64 {
    fs::read_dir(root.artifacts())
        .map(|dir| {
            dir.filter_map(|e| e.ok())
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

fn ids(sessions: &[SessionSummary]) -> Vec<String> {
    sessions.iter().map(|s| s.id.clone()).collect()
}

#[test]
fn listing_reports_ids_sizes_and_artifact_counts_from_durable_records() {
    let rt = runtime();
    let root = Root::new("list");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"one bytes");
    seed_session(&rt, &mut store, "s2", b"two bytes longer");

    let listing = rt.block_on(store.history_summary()).unwrap();
    assert_eq!(ids(&listing), vec!["s1", "s2"]);
    let s1 = listing.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(s1.artifact_count, 1);
    assert_eq!(s1.artifact_bytes, 9);
    assert_eq!(s1.event_count, 1);
    assert_eq!(s1.payload_bytes, 7);
    assert!(!s1.attached);
    assert!(s1.total_bytes() >= 9);
}

#[test]
fn listing_is_read_only_and_changes_nothing() {
    let rt = runtime();
    let root = Root::new("list-readonly");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"payload");
    let before = fingerprint(&rt, &mut store, &root);
    for _ in 0..3 {
        rt.block_on(store.history_summary()).unwrap();
    }
    assert_eq!(before, fingerprint(&rt, &mut store, &root));
}

#[test]
fn dry_run_names_the_repository_and_the_drive_path_and_leaves_state_identical() {
    let rt = runtime();
    let root = Root::new("dryrun");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"exclusive content");
    let before = fingerprint(&rt, &mut store, &root);

    let plan = rt.block_on(store.plan_purge(&["s1".to_string()])).unwrap();
    assert_eq!(plan.drive_path, root.drive().to_string_lossy());
    assert!(
        plan.repository
            .ends_with(root.0.file_name().unwrap().to_str().unwrap())
    );
    assert_eq!(plan.sessions.len(), 1);
    assert_eq!(plan.sessions[0].exclusive.len(), 1);
    assert_eq!(plan.total_bytes_released, 17);

    // The preview itself performed no deletion.
    assert_eq!(before, fingerprint(&rt, &mut store, &root));
    assert!(artifact_exists(&root, "s1-art"));
    assert!(rt.block_on(store.state("s1")).unwrap().is_some());
}

#[test]
fn exclusive_artifact_is_removed_with_its_content_file() {
    let rt = runtime();
    let root = Root::new("exclusive");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"only s1 has this");

    let outcome = rt
        .block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    assert_eq!(outcome.files_pending, Vec::<String>::new());
    assert_eq!(outcome.released_bytes, 16);
    assert!(!artifact_exists(&root, "s1-art"));
    assert!(rt.block_on(store.state("s1")).unwrap().is_none());
    assert!(rt.block_on(store.history_summary()).unwrap().is_empty());
}

#[test]
fn shared_artifact_is_preserved_under_the_surviving_holder() {
    let rt = runtime();
    let root = Root::new("shared");
    let mut store = expect_open(&rt, &root, "open");
    let shared = b"identical bytes in both sessions";
    seed_session(&rt, &mut store, "s1", shared);
    // s2 captures the same content under its own artifact id.
    rt.block_on(store.commit_unit(&[update("s2", "created")], &[event("s2", "first")]))
        .unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&capture("s2-art", "s2", shared)))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&record, &[], &[]))
        .unwrap();

    let plan = rt.block_on(store.plan_purge(&["s1".to_string()])).unwrap();
    assert!(plan.sessions[0].exclusive.is_empty());
    assert_eq!(plan.preserved_count(), 1);
    assert_eq!(
        plan.sessions[0].preserved[0].held_by,
        vec!["s2".to_string()]
    );
    assert_eq!(plan.total_bytes_released, 0);

    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();

    // The purged session is gone; the shared artifact survives, still
    // owned by a live session and still verifiable.
    assert!(rt.block_on(store.state("s1")).unwrap().is_none());
    assert!(artifact_exists(&root, "s1-art"));
    let survivor = rt
        .block_on(store.artifact_record("s2-art"))
        .unwrap()
        .unwrap();
    assert_eq!(survivor.state_id, "s2");
    assert!(matches!(
        rt.block_on(store.verify_artifact("s1-art")).unwrap(),
        crate::artifact::ArtifactStatus::Verified { .. }
    ));
    // s1's artifact row now belongs to s2, so s2 owns both copies.
    let listing = rt.block_on(store.history_summary()).unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].id, "s2");
    assert_eq!(listing[0].artifact_count, 2);
}

#[test]
fn all_purges_everything_in_the_selected_repository_only() {
    let rt = runtime();
    let root = Root::new("all-a");
    let other = Root::new("all-b");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"one");
    seed_session(&rt, &mut store, "s2", b"two");

    // A second repository present on the same filesystem, with identical
    // content so a leak would be obvious.
    let mut other_store = expect_open(&rt, &other, "open other");
    seed_session(&rt, &mut other_store, "s1", b"one");
    seed_session(&rt, &mut other_store, "s2", b"two");
    let other_before = fingerprint(&rt, &mut other_store, &other);

    let all: Vec<String> = rt
        .block_on(store.history_summary())
        .unwrap()
        .iter()
        .map(|s| s.id.clone())
        .collect();
    // With every session in the purge set nothing is "held by another
    // session outside it", so all artifacts are exclusive and removed.
    let plan = rt.block_on(store.plan_purge(&all)).unwrap();
    assert_eq!(plan.preserved_count(), 0);
    assert_eq!(plan.sessions.len(), 2);

    rt.block_on(store.purge_sessions(&all, Confirmation::ExplicitYes))
        .unwrap();
    assert!(rt.block_on(store.history_summary()).unwrap().is_empty());
    assert!(!artifact_exists(&root, "s1-art"));
    assert!(!artifact_exists(&root, "s2-art"));

    // The other repository is byte-identical: purge never scanned it.
    assert_eq!(other_before, fingerprint(&rt, &mut other_store, &other));
    assert_eq!(rt.block_on(other_store.history_summary()).unwrap().len(), 2);
}

#[test]
fn deletion_without_confirmation_refuses_and_deletes_nothing() {
    let rt = runtime();
    let root = Root::new("noconfirm");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"keep me");
    let before = fingerprint(&rt, &mut store, &root);

    let started = Instant::now();
    let error = rt
        .block_on(store.purge_sessions(&["s1".to_string()], Confirmation::None))
        .unwrap_err();
    // Prompt return, no waiting on input.
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        match error {
            PurgeError::Refused(refusal) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        },
        PurgeRefusal::ConfirmationRequired
    );
    assert_eq!(before, fingerprint(&rt, &mut store, &root));
    assert!(artifact_exists(&root, "s1-art"));
    assert!(rt.block_on(store.state("s1")).unwrap().is_some());
}

#[test]
fn confirmation_is_independent_of_yolo_and_prompted_counts_as_confirmed() {
    // The contract is that no bypass flag exists on this surface: the
    // only way to delete is Prompted or ExplicitYes. `Confirmation::None`
    // is what a YOLO caller that did not confirm gets.
    assert!(!Confirmation::None.authorizes_deletion());
    assert!(Confirmation::Prompted.authorizes_deletion());
    assert!(Confirmation::ExplicitYes.authorizes_deletion());

    let rt = runtime();
    let root = Root::new("prompted");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"confirmed away");
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::Prompted))
        .unwrap();
    assert!(rt.block_on(store.history_summary()).unwrap().is_empty());
}

#[test]
fn attached_session_is_refused_and_the_refusal_names_it() {
    let rt = runtime();
    let root = Root::new("attached");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"in use");
    seed_session(&rt, &mut store, "s2", b"idle");
    rt.block_on(store.attach_session("s1", 1_700_000_000))
        .unwrap();

    let error = rt
        .block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap_err();
    let refusal = match error {
        PurgeError::Refused(refusal) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(refusal, PurgeRefusal::SessionAttached("s1".into()));
    // The refusal names the blocking session in its message.
    assert!(refusal.to_string().contains("s1"));
    assert!(rt.block_on(store.state("s1")).unwrap().is_some());

    // The listing surfaces the attachment so the operator can see why.
    let listing = rt.block_on(store.history_summary()).unwrap();
    assert!(listing.iter().find(|s| s.id == "s1").unwrap().attached);
    assert!(!listing.iter().find(|s| s.id == "s2").unwrap().attached);

    // Detaching makes it purgeable; the other session is untouched.
    rt.block_on(store.detach_session("s1")).unwrap();
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    assert!(rt.block_on(store.state("s1")).unwrap().is_none());
    assert!(rt.block_on(store.state("s2")).unwrap().is_some());
}

#[test]
fn attachment_survives_a_reopen_and_still_blocks_purge() {
    let rt = runtime();
    let root = Root::new("attach-reopen");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"attached elsewhere");
    rt.block_on(store.attach_session("s1", 1)).unwrap();
    rt.block_on(store.close_for_test()).unwrap();

    let mut reopened = expect_open(&rt, &root, "reopen");
    assert!(rt.block_on(reopened.is_attached("s1")).unwrap());
    let error = rt
        .block_on(reopened.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap_err();
    assert_eq!(
        match error {
            PurgeError::Refused(refusal) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        },
        PurgeRefusal::SessionAttached("s1".into())
    );
}

#[test]
fn unknown_session_is_refused_without_touching_anything() {
    let rt = runtime();
    let root = Root::new("unknown");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"present");
    let before = fingerprint(&rt, &mut store, &root);

    let error = rt
        .block_on(store.purge_sessions(&["ghost".to_string()], Confirmation::ExplicitYes))
        .unwrap_err();
    assert_eq!(
        match error {
            PurgeError::Refused(refusal) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        },
        PurgeRefusal::NoSuchSession("ghost".into())
    );
    assert_eq!(before, fingerprint(&rt, &mut store, &root));
}

#[test]
fn empty_selection_is_refused_rather_than_purging_everything() {
    let rt = runtime();
    let root = Root::new("empty-selection");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"present");
    let error = rt
        .block_on(store.purge_sessions(&[], Confirmation::ExplicitYes))
        .unwrap_err();
    assert_eq!(
        match error {
            PurgeError::Refused(refusal) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        },
        PurgeRefusal::NothingToPurge
    );
    assert!(rt.block_on(store.state("s1")).unwrap().is_some());
}

#[test]
fn partial_failure_is_reported_per_item_and_retry_completes_it() {
    let rt = runtime();
    let root = Root::new("partial");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"first session");
    seed_session(&rt, &mut store, "s2", b"second session");
    // The first removal fails mid-purge; the second must still proceed.
    fail_artifact_removal_for_test(&["s1-art"]);

    let outcome = rt
        .block_on(store.purge_sessions(
            &["s1".to_string(), "s2".to_string()],
            Confirmation::ExplicitYes,
        ))
        .unwrap();
    assert_eq!(outcome.files_pending, vec!["s1-art".to_string()]);
    assert_eq!(outcome.released_bytes, 14);
    assert!(artifact_exists(&root, "s1-art"), "failed item survives");
    assert!(!artifact_exists(&root, "s2-art"), "other item completed");
    clear_artifact_removal_failures_for_test();

    // Retry: the rows are already gone, so the remainder pass finishes
    // the leftover file without touching anything else.
    let remainder = rt.block_on(store.reclaim_orphan_artifact_files()).unwrap();
    assert_eq!(remainder.reclaimed, vec!["s1-art".to_string()]);
    assert!(remainder.failed.is_empty());
    assert_eq!(remainder.reclaimed_bytes, 13);
    assert!(!artifact_exists(&root, "s1-art"));
    assert!(rt.block_on(store.history_summary()).unwrap().is_empty());
}

#[test]
fn retrying_a_completed_purge_releases_nothing_twice() {
    let rt = runtime();
    let root = Root::new("retry-idle");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"one content");
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    let after = store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap()
        .used_bytes;

    let again = rt.block_on(store.reclaim_orphan_artifact_files()).unwrap();
    assert!(again.reclaimed.is_empty());
    assert_eq!(again.reclaimed_bytes, 0);
    let still = store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap()
        .used_bytes;
    assert_eq!(after, still, "quota moved exactly once");
}

#[test]
fn purge_releases_accounted_usage_to_match_recomputed_disk_contents() {
    let rt = runtime();
    let root = Root::new("quota-release");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"a".repeat(4096).as_slice());
    seed_session(&rt, &mut store, "s2", b"b".repeat(4096).as_slice());
    let artifacts_before = artifacts_bytes(&root);
    // Checkpoint first so the journal layout is stable and the only
    // thing that can move between the two measurements is the purge.
    rt.block_on(store.checkpoint_for_test()).unwrap();
    let before = store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap()
        .used_bytes;
    assert_eq!(artifacts_before, 8192);
    assert!(before >= 8192);

    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    let artifacts_after = artifacts_bytes(&root);
    rt.block_on(store.checkpoint_for_test()).unwrap();
    let after = store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap()
        .used_bytes;

    // The purged artifact's bytes come out of the accounting, and the
    // reported total equals a fresh scan of what is actually on disk.
    // The database file itself does not shrink on DELETE: SQLite reuses
    // free pages and only VACUUM returns them, which is outside this
    // issue. The point the AC asks for is that the number tracks the
    // disk rather than a counter that could drift.
    assert_eq!(artifacts_before - artifacts_after, 4096);
    assert_eq!(before - after, 4096, "released exactly the artifact bytes");
    let db_len = fs::metadata(root.drive().join(DATABASE_FILE))
        .unwrap()
        .len();
    let wal_len = fs::metadata(root.drive().join(format!("{DATABASE_FILE}-wal")))
        .map(|m| m.len())
        .unwrap_or(0);
    let shm_len = fs::metadata(root.drive().join(format!("{DATABASE_FILE}-shm")))
        .map(|m| m.len())
        .unwrap_or(0);
    assert_eq!(
        after,
        db_len + wal_len + shm_len + artifacts_after,
        "accounting matches recomputed store contents"
    );
}

#[test]
fn a_symlinked_artifact_file_is_refused_and_never_followed() {
    let rt = runtime();
    let root = Root::new("symlink");
    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"real content");

    // Replace the artifact file with a symlink pointing at user data
    // outside the managed root.
    let outside = root.0.join("user-data.txt");
    fs::write(&outside, b"precious user data").unwrap();
    let artifact_path = root.artifacts().join("s1-art");
    fs::remove_file(&artifact_path).unwrap();
    std::os::unix::fs::symlink(&outside, &artifact_path).unwrap();

    let error = rt
        .block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    // The row removal succeeded but the file removal was refused, so the
    // symlink is reported as pending rather than followed.
    assert_eq!(error.files_pending, vec!["s1-art".to_string()]);
    assert!(outside.exists(), "symlink target was not deleted");
    assert_eq!(fs::read(&outside).unwrap(), b"precious user data");
    assert!(artifact_path.is_symlink());

    // The reclaim pass must not follow it either.
    let remainder = rt.block_on(store.reclaim_orphan_artifact_files()).unwrap();
    assert!(
        !remainder.reclaimed.contains(&"s1-art".to_string()),
        "reclaim must not remove a symlinked artifact"
    );
    assert!(outside.exists());
}

#[test]
fn reference_state_is_read_at_deletion_time_not_from_an_earlier_preview() {
    let rt = runtime();
    let root = Root::new("deletion-time");
    let mut store = expect_open(&rt, &root, "open");
    let shared = b"content two sessions share";
    seed_session(&rt, &mut store, "s1", shared);

    // Preview while s1 is the only holder: the artifact looks exclusive.
    let early = rt.block_on(store.plan_purge(&["s1".to_string()])).unwrap();
    assert_eq!(early.sessions[0].exclusive.len(), 1);
    assert_eq!(early.preserved_count(), 0);

    // A second session appears after the preview and before the delete.
    rt.block_on(store.commit_unit(&[update("s2", "created")], &[event("s2", "first")]))
        .unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&capture("s2-art", "s2", shared)))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&record, &[], &[]))
        .unwrap();

    // The delete recomputes and preserves, despite the earlier preview.
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    assert!(artifact_exists(&root, "s1-art"));
    assert!(matches!(
        rt.block_on(store.verify_artifact("s1-art")).unwrap(),
        crate::artifact::ArtifactStatus::Verified { .. }
    ));
    let listing = rt.block_on(store.history_summary()).unwrap();
    assert_eq!(ids(&listing), vec!["s2"]);
    assert_eq!(listing[0].artifact_count, 2);
}

#[test]
fn omitted_artifacts_are_removed_with_the_session_and_release_nothing() {
    let rt = runtime();
    let root = Root::new("omitted");
    let mut store = expect_open(&rt, &root, "open");
    rt.block_on(store.commit_unit(&[update("s1", "created")], &[event("s1", "first")]))
        .unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&CaptureRequest {
            spec: ArtifactSpec {
                id: "s1-omitted".into(),
                state_id: "s1".into(),
                kind: "diff".into(),
                provenance: "fixture".into(),
            },
            content: Vec::new(),
            secrets: Vec::new(),
            omitted: true,
        }))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&record, &[], &[]))
        .unwrap();

    let plan = rt.block_on(store.plan_purge(&["s1".to_string()])).unwrap();
    assert_eq!(plan.sessions[0].exclusive.len(), 1);
    assert_eq!(plan.total_bytes_released, 0);
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();
    assert!(rt.block_on(store.history_summary()).unwrap().is_empty());
}

#[test]
fn purge_never_touches_the_workspace_dot_fluzo_or_other_volumes() {
    let rt = runtime();
    let root = Root::new("workspace-safe");
    fs::write(root.0.join(".fluzo"), "x = 1\n").unwrap();
    fs::create_dir(root.0.join("src")).unwrap();
    fs::write(root.0.join("src/main.rs"), "fn main() {}").unwrap();

    let mut store = expect_open(&rt, &root, "open");
    seed_session(&rt, &mut store, "s1", b"session bytes");
    // Snapshot the workspace after the drive exists, so the comparison
    // is about what the purge did rather than what opening the store
    // created.
    let workspace_before: Vec<String> = fs::read_dir(&root.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    rt.block_on(store.purge_sessions(&["s1".to_string()], Confirmation::ExplicitYes))
        .unwrap();

    let mut workspace_after: Vec<String> = fs::read_dir(&root.0)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    workspace_after.sort();
    let mut before_sorted = workspace_before;
    before_sorted.sort();
    assert_eq!(before_sorted, workspace_after);
    assert_eq!(fs::read(root.0.join(".fluzo")).unwrap(), b"x = 1\n");
    assert_eq!(
        fs::read(root.0.join("src/main.rs")).unwrap(),
        b"fn main() {}"
    );
    // The drive itself still exists; only its session contents went.
    assert!(root.drive().exists());
    let _ = Path::new(&root.drive());
}
