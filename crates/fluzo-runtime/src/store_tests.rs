use crate::store::*;
use sqlx::{Connection, Row};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        // Canonicalize the temp base so a symlinked TMPDIR cannot make
        // the fixture root's identity differ from what the store sees,
        // and force 0700 explicitly so the runner's umask cannot widen
        // or narrow the fixture's permissions.
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-store-{}-{}",
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
    fn marker(&self) -> PathBuf {
        self.0.join("child-marker")
    }
    fn stderr_log(&self) -> PathBuf {
        self.0.join("child-stderr.log")
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn update(id: &str, payload: &str) -> StateUpdate {
    StateUpdate {
        id: id.into(),
        kind: "test".into(),
        payload: payload.into(),
    }
}

fn event(state_id: &str, payload: &str) -> EventRecord {
    EventRecord {
        state_id: state_id.into(),
        kind: "test-event".into(),
        payload: payload.into(),
    }
}

/// Re-invokes this test binary as a child that runs the crash body for the
/// given mode against the given root, then dies without cleanup.
fn spawn_child(mode: &str, root: &Root, test: &str) -> process::Child {
    // Capture child stderr AND stdout to a file so an early child exit or
    // a harness-replayed panic (libtest replays captured test output to
    // stdout) is diagnosable from the parent's failure instead of
    // vanishing into a null device.
    let stderr_file = fs::File::create(root.stderr_log()).unwrap();
    let stdout_file = stderr_file.try_clone().expect("clone child output handle");
    Command::new(std::env::current_exe().unwrap())
        .arg(test)
        .arg("--exact")
        .env("FLUZO_STORE_CHILD", mode)
        .env("FLUZO_STORE_ROOT", &root.0)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .unwrap()
}

fn child_mode() -> Option<(String, PathBuf)> {
    let mode = std::env::var("FLUZO_STORE_CHILD").ok()?;
    let root = PathBuf::from(std::env::var("FLUZO_STORE_ROOT").unwrap());
    Some((mode, root))
}

/// Opens the store with bounded backoff on environment-flake errors
/// (`Inaccessible`/`Io`/`Busy`) only. Every failed attempt is logged
/// with a full environment report before the retry, so a recovery stays
/// visible evidence. Semantic refusals are never retried.
fn expect_open(rt: &Runtime, root: &Root, context: &str) -> Store {
    let max_attempts = 4;
    let mut delay = Duration::from_millis(50);
    for attempt in 1..=max_attempts {
        match rt.block_on(Store::open(&root.0)) {
            Ok(store) => {
                if attempt > 1 {
                    crate::test_diagnostics::note_recovered(&root.0, context, attempt);
                }
                return store;
            }
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
                crate::test_diagnostics::report_and_log(
                    &root.0,
                    &format!("{context}: expected open to succeed"),
                    error,
                );
                panic!("{context}: open failed with {error:?}");
            }
        }
    }
    unreachable!("loop returns on every attempt");
}

fn wait_for_marker(child: &mut process::Child, marker: &Path, log: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !marker.exists() {
        if let Ok(Some(status)) = child.try_wait() {
            panic!(
                "child exited before signalling readiness: {status}\n--- child stderr ---\n{}",
                fs::read_to_string(log).unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "child never signalled readiness\n--- child stderr ---\n{}",
            fs::read_to_string(log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn open_enforces_wal_full_synchronous_and_foreign_keys() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    // The open path already verified the PRAGMAs; re-read them to pin the
    // durable configuration and the schema marker.
    let journal: String = rt
        .block_on(sqlx::query("PRAGMA journal_mode").fetch_one(&mut store.connection))
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    let synchronous: i64 = rt
        .block_on(sqlx::query("PRAGMA synchronous").fetch_one(&mut store.connection))
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(synchronous, 2);
    let foreign_keys: i64 = rt
        .block_on(sqlx::query("PRAGMA foreign_keys").fetch_one(&mut store.connection))
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(foreign_keys, 1);
    let version: String = rt
        .block_on(
            sqlx::query("SELECT value FROM store_meta WHERE key = 'schema_version'")
                .fetch_one(&mut store.connection),
        )
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION);
}

#[test]
fn drive_directory_is_created_private() {
    let root = Root::new();
    let rt = runtime();
    expect_open(&rt, &root, "store open");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.0.join(DRIVE_DIRECTORY))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}

#[test]
fn commit_unit_persists_state_and_events_together() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    rt.block_on(store.commit_unit(&[update("a", "1")], &[event("a", "created")]))
        .unwrap();
    let (kind, payload, revision) = rt.block_on(store.state("a")).unwrap().unwrap();
    assert_eq!(
        (kind.as_str(), payload.as_str(), revision),
        ("test", "1", 1)
    );
    rt.block_on(store.commit_unit(&[update("a", "2")], &[event("a", "updated")]))
        .unwrap();
    let (_, payload, revision) = rt.block_on(store.state("a")).unwrap().unwrap();
    assert_eq!((payload.as_str(), revision), ("2", 2));
    let events = rt.block_on(store.events_after(0, 100)).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].payload, "created");
    assert_eq!(events[1].payload, "updated");
    assert!(events[0].seq < events[1].seq);
    let tail = rt.block_on(store.events_after(events[0].seq, 100)).unwrap();
    assert_eq!(tail.len(), 1);
}

#[test]
fn foreign_key_violation_discards_the_whole_unit() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    let outcome =
        rt.block_on(store.commit_unit(&[update("kept", "x")], &[event("ghost", "dangling")]));
    assert_eq!(outcome.unwrap_err(), StoreError::Constraint);
    assert!(rt.block_on(store.state("kept")).unwrap().is_none());
    assert!(rt.block_on(store.events_after(0, 100)).unwrap().is_empty());
}

#[test]
fn interrupted_commit_rolls_back_completely() {
    let root = Root::new();
    if let Some((_, root_path)) = child_mode() {
        let rt = runtime();
        let mut store = rt.block_on(Store::open(&root_path)).unwrap();
        rt.block_on(store.commit_unit(&[update("durable", "yes")], &[event("durable", "ok")]))
            .unwrap();
        let mut tx = rt
            .block_on(store.connection.begin_with("BEGIN IMMEDIATE"))
            .unwrap();
        rt.block_on(
            sqlx::query(
                "INSERT INTO state (id, kind, payload, revision) VALUES ('torn', 'k', 'p', 1)",
            )
            .execute(&mut *tx),
        )
        .unwrap();
        rt.block_on(
            sqlx::query("INSERT INTO event (state_id, kind, payload) VALUES ('torn', 'k', 'p')")
                .execute(&mut *tx),
        )
        .unwrap();
        fs::write(root_path.join("child-marker"), "inserted").unwrap();
        std::process::abort();
    }
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    rt.block_on(store.commit_unit(&[update("before", "1")], &[event("before", "e")]))
        .unwrap();
    // Deterministic close: a dropped sqlx connection may still hold the
    // WAL index lock when the child opens, which surfaces as a spurious
    // BUSY failure only under runner scheduling.
    rt.block_on(store.close_for_test()).unwrap();
    let mut child = spawn_child(
        "torn",
        &root,
        "store_tests::interrupted_commit_rolls_back_completely",
    );
    wait_for_marker(
        &mut child,
        &root.marker(),
        &root.stderr_log(),
        Duration::from_secs(60),
    );
    let status = child.wait().unwrap();
    assert!(!status.success(), "child must die by abort");
    let mut store = expect_open(&rt, &root, "reopen after child abort");
    assert!(
        rt.block_on(store.state("torn")).unwrap().is_none(),
        "interrupted unit must not be visible"
    );
    assert!(rt.block_on(store.state("before")).unwrap().is_some());
    // The child committed 'durable' before its interrupted transaction;
    // that commit must survive even though the process aborted afterwards.
    let events = rt.block_on(store.events_after(0, 100)).unwrap();
    assert_eq!(events.iter().filter(|e| e.state_id == "torn").count(), 0);
    assert!(rt.block_on(store.state("durable")).unwrap().is_some());
}

#[test]
fn torn_wal_tail_recovers_last_complete_transaction() {
    let root = Root::new();
    if let Some((_, root_path)) = child_mode() {
        let rt = runtime();
        let mut store = rt.block_on(Store::open(&root_path)).unwrap();
        rt.block_on(store.commit_unit(
            &[update("wal-unit", "torn-target")],
            &[event("wal-unit", "e")],
        ))
        .unwrap();
        fs::write(root_path.join("child-marker"), "committed").unwrap();
        std::process::abort();
    }
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    rt.block_on(store.commit_unit(
        &[update("checkpointed", "safe")],
        &[event("checkpointed", "e")],
    ))
    .unwrap();
    rt.block_on(store.checkpoint_for_test()).unwrap();
    // Deterministic close before the child reopens (see interrupted_commit).
    rt.block_on(store.close_for_test()).unwrap();
    let mut child = spawn_child(
        "wal-torn",
        &root,
        "store_tests::torn_wal_tail_recovers_last_complete_transaction",
    );
    wait_for_marker(
        &mut child,
        &root.marker(),
        &root.stderr_log(),
        Duration::from_secs(60),
    );
    let _ = child.wait().unwrap();
    let wal = root
        .0
        .join(DRIVE_DIRECTORY)
        .join(format!("{DATABASE_FILE}-wal"));
    let length = fs::metadata(&wal).unwrap().len();
    assert!(length > 2048, "child must have left WAL frames to tear");
    Store::truncate_wal_tail_for_test(&root.0.join(DRIVE_DIRECTORY), length - 2048).unwrap();
    let mut store = expect_open(&rt, &root, "reopen after torn wal");
    // Recovery replays only complete, checksum-valid transactions: the
    // checkpointed unit survives, the torn tail unit is refused.
    assert!(rt.block_on(store.state("checkpointed")).unwrap().is_some());
    assert!(rt.block_on(store.state("wal-unit")).unwrap().is_none());
    let check: String = rt
        .block_on(sqlx::query("PRAGMA quick_check").fetch_one(&mut store.connection))
        .unwrap()
        .try_get(0)
        .unwrap();
    assert_eq!(check, "ok");
}

#[test]
fn writer_lock_is_refused_and_recovers_after_holder_dies() {
    let root = Root::new();
    if let Some((_, root_path)) = child_mode() {
        let rt = runtime();
        let mut store = rt.block_on(Store::open(&root_path)).unwrap();
        let _tx = rt.block_on(store.hold_write_lock_for_test()).unwrap();
        fs::write(root_path.join("child-marker"), "holding").unwrap();
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    let mut child = spawn_child(
        "hold",
        &root,
        "store_tests::writer_lock_is_refused_and_recovers_after_holder_dies",
    );
    wait_for_marker(
        &mut child,
        &root.marker(),
        &root.stderr_log(),
        Duration::from_secs(60),
    );
    let outcome = rt.block_on(store.commit_unit(&[update("blocked", "x")], &[]));
    assert_eq!(outcome.unwrap_err(), StoreError::Busy);
    child.kill().unwrap();
    let _ = child.wait().unwrap();
    // After the holder dies, a fresh store recovers and can commit.
    let mut store = expect_open(&rt, &root, "reopen after holder died");
    rt.block_on(store.commit_unit(&[update("after-lock", "ok")], &[event("after-lock", "e")]))
        .unwrap();
    assert!(rt.block_on(store.state("after-lock")).unwrap().is_some());
    assert!(rt.block_on(store.state("blocked")).unwrap().is_none());
}

#[test]
fn disk_full_simulation_fails_closed() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    rt.block_on(store.commit_unit(&[update("existing", "keep")], &[event("existing", "e")]))
        .unwrap();
    let pages: i64 = rt
        .block_on(sqlx::query("PRAGMA page_count").fetch_one(&mut store.connection))
        .unwrap()
        .try_get(0)
        .unwrap();
    rt.block_on(store.cap_pages_for_test((pages + 1) as u32))
        .unwrap();
    let big = "x".repeat(200_000);
    let outcome =
        rt.block_on(store.commit_unit(&[update("too-big", &big)], &[event("too-big", "e")]));
    assert_eq!(outcome.unwrap_err(), StoreError::Full);
    // Nothing half-applied: the failed unit is invisible and prior state is intact.
    assert!(rt.block_on(store.state("too-big")).unwrap().is_none());
    assert_eq!(
        rt.block_on(store.state("existing")).unwrap().unwrap().1,
        "keep"
    );
    let events = rt.block_on(store.events_after(0, 100)).unwrap();
    assert_eq!(events.iter().filter(|e| e.state_id == "too-big").count(), 0);
}

#[test]
fn symlinked_drive_directory_is_refused() {
    let root = Root::new();
    let outside = Root::new();
    std::os::unix::fs::symlink(outside.0.clone(), root.0.join(DRIVE_DIRECTORY)).unwrap();
    let rt = runtime();
    assert_eq!(
        rt.block_on(Store::open(&root.0)).unwrap_err(),
        StoreError::Conflict
    );
    assert!(!outside.0.join(DATABASE_FILE).exists());
}

#[test]
fn plain_file_named_drive_directory_is_refused() {
    let root = Root::new();
    fs::write(root.0.join(DRIVE_DIRECTORY), "not a directory").unwrap();
    let rt = runtime();
    assert_eq!(
        rt.block_on(Store::open(&root.0)).unwrap_err(),
        StoreError::Conflict
    );
}

#[test]
fn hardlinked_database_file_is_refused() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    rt.block_on(store.commit_unit(&[update("a", "1")], &[event("a", "e")]))
        .unwrap();
    drop(store);
    let db = root.0.join(DRIVE_DIRECTORY).join(DATABASE_FILE);
    fs::hard_link(&db, root.0.join(DRIVE_DIRECTORY).join("sneaky-link")).unwrap();
    assert_eq!(
        rt.block_on(Store::open(&root.0)).unwrap_err(),
        StoreError::Conflict
    );
}

#[test]
fn drive_replacement_after_open_is_refused_at_commit() {
    let root = Root::new();
    let rt = runtime();
    let mut store = expect_open(&rt, &root, "store open");
    let decoy = root.0.join("decoy-drive");
    fs::create_dir(&decoy).unwrap();
    fs::remove_dir_all(root.0.join(DRIVE_DIRECTORY)).unwrap();
    fs::rename(&decoy, root.0.join(DRIVE_DIRECTORY)).unwrap();
    assert_eq!(
        rt.block_on(store.commit_unit(&[update("a", "1")], &[]))
            .unwrap_err(),
        StoreError::Conflict
    );
}
