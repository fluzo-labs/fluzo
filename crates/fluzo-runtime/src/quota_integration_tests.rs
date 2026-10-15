use crate::artifact::{ARTIFACTS_DIRECTORY, ArtifactSpec, CaptureRequest};
use crate::quota::*;
use crate::store::*;
use sha2::{Digest, Sha256};
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
        let path = std::env::temp_dir().join(format!(
            "fluzo-quota-{tag}-{}-{}",
            process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn drive(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY)
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

fn open_with(root: &Root, limits: QuotaLimits) -> Store {
    runtime()
        .block_on(Store::open_quotated_with_probe(
            &root.0,
            limits,
            FixedFreeSpace(1 << 40),
        ))
        .unwrap()
}

/// Closes a store deterministically and reopens it under `limits`.
///
/// `drop` is not enough here. Dropping a sqlx connection defers the real
/// close to a background thread, so a reopen issued immediately after a
/// `drop` can race the still-held WAL index lock and fail with
/// SQLITE_BUSY under `busy_timeout = 0`. #81 added `close_for_test` for
/// exactly this handoff, so the quota reopen fixtures close explicitly
/// instead of relying on drop timing.
fn close_and_reopen(store: Store, root: &Root, limits: QuotaLimits) -> Store {
    runtime().block_on(store.close_for_test()).unwrap();
    open_with(root, limits)
}

/// Opens a store whose session threshold sits `session_extra` bytes above
/// the real size of the freshly created journal, so thresholds are
/// crossed with small writes rather than gigabytes. Returns the store and
/// the baseline it was calibrated against.
fn open_sized(root: &Root, session_extra: u64, diagnostics: u64) -> (Store, u64) {
    let mut store = open_with(root, QuotaLimits::default());
    let baseline = session_used(&store);
    store
        .set_quota_limits_for_test(QuotaLimits {
            session_max_bytes: baseline + session_extra,
            diagnostic_max_bytes: diagnostics,
            min_free_bytes: 1,
        })
        .unwrap();
    (store, baseline)
}

fn session_used(store: &Store) -> u64 {
    store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap()
        .used_bytes
}

fn diagnostics_used(store: &Store) -> u64 {
    store
        .quota_status(QuotaClass::Diagnostics)
        .unwrap()
        .unwrap()
        .used_bytes
}

/// Independent recomputation of one directory's managed bytes, used to
/// prove the ledger counts exactly what is on disk.
fn files_under(dir: &Path) -> u64 {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.path().symlink_metadata().unwrap();
                if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                }
            })
            .sum(),
        Err(_) => 0,
    }
}

fn journal_bytes(drive: &Path) -> u64 {
    [
        DATABASE_FILE,
        &format!("{DATABASE_FILE}-wal"),
        &format!("{DATABASE_FILE}-shm"),
    ]
    .iter()
    .map(|name| {
        fs::symlink_metadata(drive.join(name))
            .map(|metadata| metadata.len())
            .unwrap_or(0)
    })
    .sum()
}

/// Every name under the drive root. Used to prove nothing is removed.
fn drive_names(root: &Root) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.drive()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = path
                .strip_prefix(root.drive())
                .unwrap()
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// Content hashes of the retained managed files. SQLite's own journal
/// files are deliberately excluded: their bytes legitimately change on
/// recovery and checkpoint, which is not eviction. The logical contents
/// of the database are asserted separately.
fn managed_inventory(root: &Root) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for class in [ARTIFACTS_DIRECTORY, DIAGNOSTICS_DIRECTORY] {
        let dir = root.drive().join(class);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = format!("{class}/{}", entry.file_name().to_string_lossy());
            let mut hasher = Sha256::new();
            hasher.update(fs::read(&path).unwrap());
            let digest = hasher.finalize();
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            out.push((name, hex));
        }
    }
    out.sort();
    out
}

fn snapshot(root: &Root) -> (Vec<String>, Vec<(String, String)>) {
    (drive_names(root), managed_inventory(root))
}

/// Fixture writer that grows the session class by a chosen amount, so a
/// test crosses a threshold without writing the real limit.
fn pad_session(root: &Root, bytes: usize) {
    let artifacts = root.drive().join(ARTIFACTS_DIRECTORY);
    fs::create_dir_all(&artifacts).unwrap();
    fs::write(artifacts.join("pad.bin"), vec![b'p'; bytes]).unwrap();
}

#[test]
fn a_refused_unit_leaves_no_partial_audit_state() {
    let root = Root::new("refuse");
    let (mut store, _) = open_sized(&root, 128_000, 100_000);
    runtime()
        .block_on(store.commit_unit(&[update("keep", "kept")], &[event("keep", "kept")]))
        .unwrap();

    pad_session(&root, 100_000);
    let used = session_used(&store);

    let error = runtime()
        .block_on(store.commit_unit(&[update("ghost", "never")], &[event("ghost", "never")]))
        .unwrap_err();
    assert_eq!(
        error,
        StoreError::QuotaReached {
            class: QuotaClass::SessionData,
            used,
            limit: store.quota_limits().unwrap().session_max_bytes,
        }
    );

    // Nothing half-applied: the ghost rows do not exist and the kept
    // unit is intact and readable.
    assert!(runtime().block_on(store.state("ghost")).unwrap().is_none());
    let kept = runtime().block_on(store.state("keep")).unwrap().unwrap();
    assert_eq!(kept, ("test".to_string(), "kept".to_string(), 1));
    let events = runtime().block_on(store.events_after(0, 100)).unwrap();
    assert!(
        events.iter().all(|event| event.state_id != "ghost"),
        "a refused unit must not journal anything"
    );
}

#[test]
fn the_warning_is_observable_before_the_refusal() {
    let root = Root::new("warn");
    let (mut store, baseline) = open_sized(&root, 500_000, 100_000);
    let limit = baseline + 500_000;

    // Sit just above the warning threshold but far enough below the
    // refusal point that a small unit still commits.
    pad_session(&root, (limit * 85 / 100 - baseline) as usize);
    let status = store
        .quota_status(QuotaClass::SessionData)
        .unwrap()
        .unwrap();
    assert!(status.warning, "80% utilization must warn");
    assert!(
        status.used_bytes < status.limit_bytes,
        "the warning must precede, not replace, the refusal"
    );

    runtime()
        .block_on(store.commit_unit(&[update("a", "x")], &[]))
        .unwrap();
    assert!(matches!(
        store.last_admission(),
        Some(Admission::Warning(_))
    ));

    // Close the remaining gap and the same call is refused, with the
    // observable outcome moved to the refusal variant.
    pad_session(&root, (limit - 60_000 - baseline) as usize);
    let error = runtime()
        .block_on(store.commit_unit(&[update("b", "y")], &[]))
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::QuotaReached {
            class: QuotaClass::SessionData,
            ..
        }
    ));
    assert!(matches!(
        store.last_admission(),
        Some(Admission::Refused(_))
    ));
}

#[test]
fn diagnostics_saturate_by_dropping_with_visible_counters() {
    let root = Root::new("diag");
    let (mut store, _) = open_sized(&root, 10_000_000, 1_000);
    assert!(
        store
            .append_diagnostic("a.log", &[b'a'; 400])
            .unwrap()
            .accepted()
    );
    assert!(
        store
            .append_diagnostic("b.log", &[b'b'; 400])
            .unwrap()
            .accepted()
    );
    let dropped = store.append_diagnostic("c.log", &[b'c'; 300]).unwrap();
    assert!(matches!(
        dropped,
        DiagnosticDisposition::Dropped {
            reason: QuotaViolation::DiagnosticLimit,
            ..
        }
    ));

    let counters = store.diagnostic_counters();
    assert_eq!(counters.written_records, 2);
    assert_eq!(counters.written_bytes, 800);
    assert_eq!(counters.dropped_records, 1);
    assert_eq!(counters.dropped_bytes, 300);

    // Existing capture is retained byte for byte; nothing was evicted to
    // make room for the new record.
    let telemetry = root.drive().join(DIAGNOSTICS_DIRECTORY);
    let names: Vec<String> = fs::read_dir(&telemetry)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"a.log".to_string()));
    assert!(names.contains(&"b.log".to_string()));
    assert_eq!(fs::read(telemetry.join("a.log")).unwrap(), vec![b'a'; 400]);
    assert!(!telemetry.join("c.log").exists());
}

#[test]
fn diagnostic_saturation_does_not_block_ordinary_execution() {
    let root = Root::new("diagblock");
    let (mut store, _) = open_sized(&root, 10_000_000, 100);
    store.append_diagnostic("a.log", &[b'a'; 90]).unwrap();
    assert!(
        !store
            .append_diagnostic("b.log", &[b'b'; 90])
            .unwrap()
            .accepted()
    );
    // Ordinary state-mutating work continues with diagnostics saturated.
    runtime()
        .block_on(store.commit_unit(&[update("work", "still running")], &[event("work", "ok")]))
        .unwrap();
    assert_eq!(store.diagnostic_counters().dropped_records, 1);
    assert!(runtime().block_on(store.state("work")).unwrap().is_some());
}

#[test]
fn a_mandatory_audit_failure_still_blocks_the_task() {
    let root = Root::new("audit");
    let (mut store, _) = open_sized(&root, 10_000_000, 10_000_000);
    // Diagnostics are healthy; the durable audit journal cannot take the
    // write, so the unit must not be applied.
    runtime().block_on(store.cap_pages_for_test(2)).unwrap();
    let error = runtime()
        .block_on(store.commit_unit(&[update("big", &"z".repeat(9_000))], &[]))
        .unwrap_err();
    assert_eq!(error, StoreError::Full);
    assert!(runtime().block_on(store.state("big")).unwrap().is_none());
    assert_eq!(store.diagnostic_counters().dropped_records, 0);
    assert_eq!(diagnostics_used(&store), 0);
}

#[test]
fn restart_recomputes_the_same_usage_including_wal_and_artifacts() {
    let root = Root::new("restart");
    let (mut store, _) = open_sized(&root, 10_000_000, 100_000);
    runtime()
        .block_on(store.commit_unit(&[update("s1", "payload-one")], &[event("s1", "e1")]))
        .unwrap();
    let record = runtime()
        .block_on(store.finalize_artifact(&capture("art1", "s1", b"artifact content")))
        .unwrap();
    runtime()
        .block_on(store.commit_artifact_reference(
            &record,
            &[update("s1", "payload-two")],
            &[event("s1", "e2")],
        ))
        .unwrap();
    store
        .append_diagnostic("run.log", b"diagnostic line")
        .unwrap();

    // The ledger counts exactly the managed files, WAL side files
    // included, at this instant.
    let artifacts = files_under(&root.drive().join(ARTIFACTS_DIRECTORY));
    let telemetry = files_under(&root.drive().join(DIAGNOSTICS_DIRECTORY));
    assert_eq!(
        session_used(&store),
        journal_bytes(&root.drive()) + artifacts
    );
    assert_eq!(diagnostics_used(&store), telemetry);
    assert!(artifacts > 0 && telemetry > 0);

    let limits = store.quota_limits().unwrap();
    let reopened = close_and_reopen(store, &root, limits);
    assert_eq!(diagnostics_used(&reopened), telemetry);
    assert_eq!(
        session_used(&reopened),
        journal_bytes(&root.drive()) + artifacts
    );

    // A second reopen agrees exactly with the first: the recomputation
    // is stable, not drifting.
    let again = close_and_reopen(reopened, &root, limits);
    assert_eq!(
        session_used(&again),
        journal_bytes(&root.drive()) + artifacts
    );
    assert_eq!(diagnostics_used(&again), telemetry);
}

#[test]
fn two_repositories_are_quota_independent() {
    let a = Root::new("proj-a");
    let b = Root::new("proj-b");
    let (mut store_a, _) = open_sized(&a, 128_000, 1_000);
    let (mut store_b, _) = open_sized(&b, 500_000, 100_000);

    runtime()
        .block_on(store_b.commit_unit(&[update("b0", "before")], &[]))
        .unwrap();
    let b_usage = session_used(&store_b);
    let b_limit = store_b.quota_limits().unwrap().session_max_bytes;

    // Drive A to a session refusal and saturate its diagnostics.
    pad_session(&a, 100_000);
    assert!(
        runtime()
            .block_on(store_a.commit_unit(&[update("a1", "x")], &[]))
            .is_err()
    );
    store_a.append_diagnostic("a.log", &[b'a'; 900]).unwrap();
    assert!(
        !store_a
            .append_diagnostic("b.log", &[b'b'; 900])
            .unwrap()
            .accepted()
    );

    // B is untouched: same limit, same usage, still admits its own work.
    assert_eq!(store_b.quota_limits().unwrap().session_max_bytes, b_limit);
    assert_eq!(session_used(&store_b), b_usage);
    runtime()
        .block_on(store_b.commit_unit(&[update("b1", "fine")], &[event("b1", "fine")]))
        .unwrap();
    assert_eq!(store_b.diagnostic_counters().dropped_records, 0);
    assert!(session_used(&store_b) > b_usage, "B gained its own records");

    // A's files never entered B's accounting.
    assert!(session_used(&store_b) < b_limit);
}

#[test]
fn nothing_is_evicted_by_pressure_time_or_reopen() {
    let root = Root::new("evict");
    let (mut store, baseline) = open_sized(&root, 500_000, 100);
    let limits = store.quota_limits().unwrap();
    runtime()
        .block_on(store.commit_unit(&[update("keep", "retained")], &[event("keep", "retained")]))
        .unwrap();
    let record = runtime()
        .block_on(store.finalize_artifact(&capture("art1", "keep", b"retained artifact")))
        .unwrap();
    runtime()
        .block_on(store.commit_artifact_reference(&record, &[update("keep", "retained")], &[]))
        .unwrap();
    store
        .append_diagnostic("keep.log", b"retained diagnostic")
        .unwrap();

    // Saturate the session class, then take the inventory to preserve.
    pad_session(
        &root,
        (limits.session_max_bytes - baseline - 30_000) as usize,
    );
    let before = snapshot(&root);
    assert!(before.1.len() >= 3);

    // Quota pressure: repeated refused admissions.
    for _ in 0..10 {
        assert!(
            runtime()
                .block_on(store.commit_unit(&[update("ghost", "no")], &[]))
                .is_err()
        );
    }
    // Diagnostic saturation: every further record is dropped, not
    // evicted-over.
    for index in 0..10 {
        let _ = store.append_diagnostic(&format!("drop{index}.log"), &[b'd'; 900]);
    }
    assert_eq!(store.diagnostic_counters().dropped_records, 10);
    assert_eq!(snapshot(&root), before, "pressure evicted nothing");

    // Elapsed time: retention is not age-based.
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    assert_eq!(snapshot(&root), before, "elapsed time evicted nothing");

    // Reopen under the same saturated limits.
    let mut reopened = close_and_reopen(store, &root, limits);
    assert_eq!(
        (drive_names(&root), managed_inventory(&root)),
        before,
        "reopen evicted nothing"
    );
    assert_eq!(
        diagnostics_used(&reopened),
        19,
        "only the retained keep.log survives"
    );
    assert!(
        runtime()
            .block_on(reopened.state("keep"))
            .unwrap()
            .is_some()
    );
    // Still saturated after reopen: pressure continues, retention holds.
    for index in 10..13 {
        let _ = reopened.append_diagnostic(&format!("drop{index}.log"), &[b'd'; 900]);
    }
    assert_eq!(reopened.diagnostic_counters().dropped_records, 3);
    assert_eq!(managed_inventory(&root), before.1);
    // The kept audit content survives the saturated reopen unchanged.
    assert_eq!(
        runtime()
            .block_on(reopened.state("keep"))
            .unwrap()
            .unwrap()
            .1,
        "retained"
    );
}

#[test]
fn the_bounded_diagnostic_contract_requires_an_attached_guard() {
    let root = Root::new("noguard");
    let mut store = runtime().block_on(Store::open(&root.0)).unwrap();
    assert_eq!(
        store.append_diagnostic("a.log", b"x").unwrap_err(),
        StoreError::QuotaNotConfigured
    );
    // The unguarded store still behaves exactly as before for its own work.
    runtime()
        .block_on(store.commit_unit(&[update("plain", "ok")], &[]))
        .unwrap();
    assert!(
        store
            .quota_status(QuotaClass::SessionData)
            .unwrap()
            .is_none()
    );
}

#[test]
fn unsafe_diagnostic_record_names_are_refused() {
    let root = Root::new("names");
    let (mut store, _) = open_sized(&root, 10_000_000, 100_000);
    for name in [
        "",
        ".hidden",
        "../escape",
        "sub/dir.log",
        "with space.log",
        &"x".repeat(129),
    ] {
        assert_eq!(
            store.append_diagnostic(name, b"x").unwrap_err(),
            StoreError::UnsupportedPath,
            "{name:?} must not be accepted as a record name"
        );
    }
    assert!(
        store
            .append_diagnostic("run-1.log", b"ok")
            .unwrap()
            .accepted()
    );
}

#[test]
fn artifact_bytes_are_admitted_before_they_land_on_disk() {
    let root = Root::new("artifact-admission");
    let (mut store, baseline) = open_sized(&root, 500_000, 100_000);
    let limit = baseline + 500_000;
    runtime()
        .block_on(store.commit_unit(&[update("s", "x")], &[]))
        .unwrap();
    // Leave less than the artifact plus its audit headroom.
    pad_session(&root, (limit - baseline - 70_000) as usize);
    let error = runtime()
        .block_on(store.finalize_artifact(&capture("big", "s", &[b'q'; 5_000])))
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::QuotaReached {
            class: QuotaClass::SessionData,
            ..
        }
    ));
    // The refused content never appeared in the managed artifact tree.
    assert!(!root.drive().join(ARTIFACTS_DIRECTORY).join("big").exists());
}
