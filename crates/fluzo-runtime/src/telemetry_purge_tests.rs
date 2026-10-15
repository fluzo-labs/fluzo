//! DAT-02e deterministic tests for the local telemetry archive purge.
//!
//! Every test uses a private temporary drive and named sinks. The purge
//! is synchronous; the hub is thread-based. No test touches a real
//! repository, network or the user's home.

use crate::store::Store;
use crate::telemetry::{
    DiagnosticHub, DiagnosticSink, HubLimits, JsonlArchiveSink, RedactionPolicy,
};
use crate::telemetry_archive::JsonlArchive;
use crate::telemetry_purge::{PurgeConfirmation, PurgeError, preview, purge};
use fluzo_core::telemetry::{
    DiagnosticPayload, DiagnosticRecord, LogLevel, LogPayload, ResourceIdentity, SignalKind,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);

impl Root {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("fluzo-purge-{label}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create root");
        Self(path)
    }
    fn telemetry(&self) -> PathBuf {
        self.0.join(".fluzodrive").join("telemetry")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn record(message: &str) -> DiagnosticRecord {
    DiagnosticRecord::new(
        SignalKind::Log,
        1_700_000_000_000,
        ResourceIdentity {
            service_name: "fluzo-test".into(),
            service_version: "0.0.1".into(),
        },
        None,
        DiagnosticPayload::Log(LogPayload {
            level: LogLevel::Info,
            message: message.into(),
            attributes: Default::default(),
        }),
    )
}

/// Sink whose `write_batch` blocks while the test holds `gate`.
struct BlockedSink {
    sink_name: String,
    gate: Arc<Mutex<()>>,
}
impl DiagnosticSink for BlockedSink {
    fn name(&self) -> String {
        self.sink_name.clone()
    }
    fn write_batch(&mut self, _lines: &[String]) -> Result<(), String> {
        let _guard = self.gate.lock().unwrap();
        Ok(())
    }
}

/// Sink that accepts everything immediately.
struct DrainSink {
    sink_name: String,
}
impl DiagnosticSink for DrainSink {
    fn name(&self) -> String {
        self.sink_name.clone()
    }
    fn write_batch(&mut self, _lines: &[String]) -> Result<(), String> {
        Ok(())
    }
}

fn hub_with(sink: Box<dyn DiagnosticSink>) -> DiagnosticHub {
    let mut hub = DiagnosticHub::new(HubLimits::default(), RedactionPolicy::default());
    hub.add_sink(sink);
    hub
}

fn sha_like(bytes: &[u8]) -> u128 {
    let mut hash: u128 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u128;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn drive_fingerprint(drive: &Path) -> Vec<(String, u128)> {
    let mut files = Vec::new();
    for name in ["state.sqlite3", "state.sqlite3-wal", "state.sqlite3-shm"] {
        let path = drive.join(name);
        if let Ok(bytes) = fs::read(&path) {
            files.push((name.to_owned(), sha_like(&bytes)));
        }
    }
    files.sort();
    files
}

fn listing(dir: &Path) -> HashSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

// ---- preview ----

#[test]
fn dry_run_previews_archive_and_pending_buffers_with_byte_counts() {
    let root = Root::new("preview");
    let telemetry = root.telemetry();
    write_file(
        &telemetry.join("segment-000000.jsonl"),
        b"{\"a\":1}\n{\"b\":2}\n",
    );
    write_file(&telemetry.join("segment-000001.jsonl"), b"{\"c\":3}\n");
    write_file(&telemetry.join("diag-1000-aaaa.json"), b"{\"pending\":1}\n");
    write_file(&telemetry.join("diag-1001-bbbb.json"), b"{\"pending\":2}\n");

    let p = preview(&root.0, None).unwrap();
    assert_eq!(p.archive_segments, 2, "two segment files");
    assert_eq!(p.archive_bytes, 16 + 8, "archive bytes");
    assert_eq!(p.pending_buffers, 2, "two pending buffer files");
    assert_eq!(p.pending_bytes, 14 + 14, "pending bytes");
    assert_eq!(p.repository, root.0.file_name().unwrap().to_string_lossy());
    assert_eq!(p.telemetry_path, telemetry);
    assert_eq!(p.total_bytes(), p.archive_bytes + p.pending_bytes);
}

#[test]
fn preview_is_zero_and_creates_nothing_when_no_telemetry() {
    let root = Root::new("preview-empty");
    let p = preview(&root.0, None).unwrap();
    assert_eq!(p.archive_segments, 0);
    assert_eq!(p.pending_buffers, 0);
    assert_eq!(p.total_bytes(), 0);
    assert!(
        !root.telemetry().exists(),
        "preview must not create the telemetry directory"
    );
    assert!(
        !root.0.join(".fluzodrive").exists(),
        "preview must not create the drive"
    );
}

#[test]
fn dry_run_leaves_the_tree_byte_for_byte_unchanged() {
    let root = Root::new("dry-pure");
    let telemetry = root.telemetry();
    write_file(&telemetry.join("segment-000000.jsonl"), b"{\"a\":1}\n");
    write_file(&telemetry.join("diag-1.json"), b"{\"p\":1}\n");
    let before = listing(&telemetry);

    let error = purge(
        &root.0,
        None,
        PurgeConfirmation::DryRun,
        Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(error, PurgeError::ConfirmationRequired);

    assert_eq!(before, listing(&telemetry), "dry-run changed nothing");
}

// ---- confirmed purge ----

#[test]
fn confirmed_purge_removes_archive_and_pending_buffers() {
    let root = Root::new("purge-all");
    let telemetry = root.telemetry();
    write_file(
        &telemetry.join("segment-000000.jsonl"),
        b"{\"a\":1}\n{\"b\":2}\n",
    );
    write_file(&telemetry.join("diag-1.json"), b"{\"p\":1}\n");

    let result = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(result.archive_segments_removed, 1);
    assert_eq!(result.pending_buffers_removed, 1);
    assert_eq!(result.archive_bytes_removed, 16);
    assert_eq!(result.pending_bytes_removed, 8);
    assert!(
        listing(&telemetry).is_empty(),
        "telemetry emptied: {:?}",
        listing(&telemetry)
    );
}

#[test]
fn purge_is_idempotent_second_run_removes_nothing() {
    let root = Root::new("idempotent");
    write_file(
        &root.telemetry().join("segment-000000.jsonl"),
        b"{\"a\":1}\n",
    );
    let first = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(first.archive_segments_removed, 1);
    let second = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(second.archive_segments_removed, 0);
    assert_eq!(second.pending_buffers_removed, 0);
    assert_eq!(
        second.diagnostics_used_after, 0,
        "second purge sees an empty diagnostics class"
    );
}

#[test]
fn purge_result_states_what_was_not_cleared() {
    let root = Root::new("not-cleared");
    write_file(
        &root.telemetry().join("segment-000000.jsonl"),
        b"{\"a\":1}\n",
    );
    let result = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    let rendered = result.to_string();
    assert!(
        rendered.contains("remote backend copies already exported"),
        "result must state remote data is not cleared: {rendered}"
    );
    assert!(
        rendered.contains("shared aggregates"),
        "result must state shared aggregates are not cleared: {rendered}"
    );
    assert!(
        !rendered.to_lowercase().contains("erased remote"),
        "must not overclaim: {rendered}"
    );
}

// ---- writer quiesce ----

#[test]
fn purge_refuses_under_live_writer_and_deletes_nothing() {
    let root = Root::new("live-writer");
    let telemetry = root.telemetry();
    write_file(&telemetry.join("segment-000000.jsonl"), b"{\"a\":1}\n");
    write_file(&telemetry.join("diag-1.json"), b"{\"p\":1}\n");

    let gate = Arc::new(Mutex::new(()));
    let held = gate.lock().unwrap();
    let mut hub = hub_with(Box::new(BlockedSink {
        sink_name: "stuck-exporter".into(),
        gate: Arc::clone(&gate),
    }));
    // Put a record in the blocked sink so its worker is mid-write.
    let _ = hub.publish(record("queued"));

    let error = purge(
        &root.0,
        Some(&mut hub),
        PurgeConfirmation::Yes,
        Duration::from_millis(200),
    )
    .expect_err("a stuck writer must refuse the purge");
    match error {
        PurgeError::ActiveWriter { component } => {
            assert_eq!(component, "stuck-exporter", "names the active component");
        }
        other => panic!("expected ActiveWriter, got {other:?}"),
    }

    // Nothing deleted.
    assert!(telemetry.join("segment-000000.jsonl").exists());
    assert!(telemetry.join("diag-1.json").exists());

    // Release the gate so the detached worker thread exits cleanly.
    drop(held);
}

#[test]
fn quiesced_writer_allows_purge_after_drain() {
    let root = Root::new("quiesce-ok");
    write_file(
        &root.telemetry().join("segment-000000.jsonl"),
        b"{\"a\":1}\n",
    );

    let mut hub = hub_with(Box::new(DrainSink {
        sink_name: "healthy".into(),
    }));
    let _ = hub.publish(record("queued"));

    let result = purge(
        &root.0,
        Some(&mut hub),
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .expect("a drainable writer must allow the purge");
    assert_eq!(result.archive_segments_removed, 1);
    assert!(!root.telemetry().join("segment-000000.jsonl").exists());
}

#[test]
fn drained_producer_cannot_resurrect_old_records() {
    let root = Root::new("resurrect");
    fs::create_dir_all(root.telemetry()).unwrap();

    // Old records in the archive.
    let mut old_archive = JsonlArchive::open(&root.0).unwrap();
    old_archive
        .append_batch(&["{\"old\":1}".to_string()])
        .unwrap();
    assert!(root.telemetry().join("segment-000000.jsonl").exists());

    // A hub writing to that archive, then quiesced.
    let mut hub = hub_with(Box::new(JsonlArchiveSink::new(
        JsonlArchive::open(&root.0).unwrap(),
    )));
    let _ = hub.publish(record("old-two"));
    let report = hub.shutdown(Duration::from_secs(2));
    assert!(report.incomplete_flushes.is_empty());

    // Purge clears everything.
    let result = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(result.archive_segments_removed >= 1);
    assert!(!root.telemetry().join("segment-000000.jsonl").exists());

    // The drained hub cannot publish again.
    let outcome = hub.publish(record("ghost"));
    assert_eq!(outcome.dropped, 1, "a drained producer cannot write");

    // A new producer creates clearly-new records only.
    let mut new_archive = JsonlArchive::open(&root.0).unwrap();
    new_archive
        .append_batch(&["{\"new\":1}".to_string()])
        .unwrap();
    let contents = fs::read_to_string(root.telemetry().join("segment-000000.jsonl")).unwrap();
    assert!(contents.contains("\"new\":1"));
    assert!(
        !contents.contains("old"),
        "old records must not reappear: {contents}"
    );
}

// ---- independence from history / database ----

#[test]
fn telemetry_purge_does_not_touch_the_database() {
    let root = Root::new("db-untouched");
    let drive = root.0.join(".fluzodrive");
    fs::create_dir_all(&drive).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::open(&root.0)).expect("open store");
    // Deterministic close: sqlx defers the real close to a background
    // thread, so a plain drop leaves the WAL checkpoint racing the
    // fingerprint. Close explicitly before snapshotting (same discipline
    // as the T16 store-reopen hardening).
    runtime
        .block_on(store.close_for_test())
        .expect("close store");

    // Telemetry files written directly (archive + pending buffer).
    let mut archive = JsonlArchive::open(&root.0).unwrap();
    archive.append_batch(&["{\"seg\":1}".to_string()]).unwrap();
    write_file(&root.telemetry().join("diag-1.json"), b"{\"pending\":1}\n");

    let db_before = drive_fingerprint(&drive);
    assert!(!db_before.is_empty(), "database files exist to compare");

    let result = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(result.archive_segments_removed >= 1);
    assert!(result.pending_buffers_removed >= 1);

    let db_after = drive_fingerprint(&drive);
    assert_eq!(
        db_before, db_after,
        "telemetry purge must not modify the database"
    );
    assert!(listing(&root.telemetry()).is_empty(), "telemetry emptied");
    assert!(
        drive.join("state.sqlite3").exists(),
        "database still present"
    );
}

// ---- unsafe root ----

#[test]
fn purge_rejects_symlinked_entry_inside_root() {
    let root = Root::new("symlink-entry");
    let telemetry = root.telemetry();
    fs::create_dir_all(&telemetry).unwrap();
    write_file(&telemetry.join("segment-000000.jsonl"), b"{\"a\":1}\n");
    let outside = root.0.join("outside.json");
    write_file(&outside, b"{\"secret\":1}\n");
    std::os::unix::fs::symlink(&outside, telemetry.join("diag-link.json")).unwrap();

    let error = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error, PurgeError::UnsafeRoot);
    assert!(outside.exists(), "outside file untouched");
    assert!(
        telemetry.join("segment-000000.jsonl").exists(),
        "nothing deleted"
    );
}

#[test]
fn purge_rejects_symlinked_telemetry_root() {
    let root = Root::new("symlink-root");
    let real = root.0.join("real-telemetry");
    fs::create_dir_all(&real).unwrap();
    write_file(&real.join("segment-000000.jsonl"), b"{\"a\":1}\n");
    fs::create_dir_all(root.0.join(".fluzodrive")).unwrap();
    std::os::unix::fs::symlink(&real, root.telemetry()).unwrap();

    let error = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error, PurgeError::UnsafeRoot);
    assert!(
        real.join("segment-000000.jsonl").exists(),
        "real data untouched"
    );
}

#[test]
fn preview_also_rejects_unsafe_root() {
    let root = Root::new("preview-unsafe");
    let telemetry = root.telemetry();
    fs::create_dir_all(&telemetry).unwrap();
    let outside = root.0.join("outside");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, telemetry.join("nested")).unwrap();
    let error = preview(&root.0, None).unwrap_err();
    assert_eq!(error, PurgeError::UnsafeRoot);
}

// ---- accounting ----

#[test]
fn purge_releases_diagnostics_accounting() {
    let root = Root::new("accounting");
    let telemetry = root.telemetry();
    write_file(
        &telemetry.join("segment-000000.jsonl"),
        b"{\"a\":1}\n{\"b\":2}\n",
    );
    let before = preview(&root.0, None).unwrap();
    let result = purge(
        &root.0,
        None,
        PurgeConfirmation::Yes,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(
        result.diagnostics_used_before,
        before.total_bytes(),
        "before matches the previewed bytes"
    );
    assert_eq!(result.diagnostics_used_after, 0, "after is empty");
    assert!(
        result.diagnostics_used_before > result.diagnostics_used_after,
        "accounting released"
    );
}
