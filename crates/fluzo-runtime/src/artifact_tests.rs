use crate::artifact::*;
use crate::store::{EventRecord, StateUpdate, Store, StoreError};
use sha2::{Digest, Sha256};
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
        let path = std::env::temp_dir().join(format!(
            "fluzo-artifact-{}-{}",
            process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn marker(&self) -> PathBuf {
        self.0.join("child-marker")
    }
    fn stderr_log(&self) -> PathBuf {
        self.0.join("child-stderr.log")
    }
    fn artifacts(&self) -> PathBuf {
        self.0.join(".fluzodrive").join(ARTIFACTS_DIRECTORY)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn spawn_child(mode: &str, root: &Root, test: &str) -> process::Child {
    // Capture child stderr to a file so an early child exit is diagnosable
    // from the parent's failure instead of vanishing into a null device.
    let stderr_file = fs::File::create(root.stderr_log()).unwrap();
    Command::new(std::env::current_exe().unwrap())
        .arg(test)
        .arg("--exact")
        .env("FLUZO_ARTIFACT_CHILD", mode)
        .env("FLUZO_ARTIFACT_ROOT", &root.0)
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .unwrap()
}

fn child_mode() -> Option<(String, PathBuf)> {
    let mode = std::env::var("FLUZO_ARTIFACT_CHILD").ok()?;
    let root = PathBuf::from(std::env::var("FLUZO_ARTIFACT_ROOT").unwrap());
    Some((mode, root))
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

fn spec(id: &str) -> ArtifactSpec {
    ArtifactSpec {
        id: id.into(),
        state_id: format!("{id}-owner"),
        kind: "tool-output".into(),
        provenance: "fixture:unit".into(),
    }
}

fn request(id: &str, content: &[u8]) -> CaptureRequest {
    CaptureRequest {
        spec: spec(id),
        content: content.to_vec(),
        secrets: Vec::new(),
        omitted: false,
    }
}

fn owner_update(id: &str) -> StateUpdate {
    StateUpdate {
        id: format!("{id}-owner"),
        kind: "owner".into(),
        payload: "live".into(),
    }
}

fn created_event(id: &str) -> EventRecord {
    EventRecord {
        state_id: format!("{id}-owner"),
        kind: "artifact-created".into(),
        payload: id.to_string(),
    }
}

fn independent_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn capture_finalizes_content_before_reference_commit() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("a1", b"hello artifact")))
        .unwrap();
    // Finalization wrote the content file but nothing is committed yet.
    assert_eq!(
        fs::read(root.artifacts().join("a1")).unwrap(),
        b"hello artifact"
    );
    assert!(rt.block_on(store.artifact_record("a1")).unwrap().is_none());
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("a1")],
        &[created_event("a1")],
    ))
    .unwrap();
    let stored = rt.block_on(store.artifact_record("a1")).unwrap().unwrap();
    assert_eq!(stored, record);
    assert_eq!(stored.stored_bytes, 14);
    assert_eq!(stored.state_id, "a1-owner");
    assert_eq!(stored.provenance, "fixture:unit");
    assert!(stored.flags.is_empty());
    assert_eq!(
        rt.block_on(store.verify_artifact("a1")).unwrap(),
        ArtifactStatus::Verified {
            stored_bytes: 14,
            content_hash: record.content_hash.clone(),
        }
    );
}

#[test]
fn stored_bytes_are_independently_verified_at_byte_level() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let payload = b"raw bytes 0123456789 \x00\xff binary".to_vec();
    let record = rt
        .block_on(store.finalize_artifact(&request("bytes1", &payload)))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("bytes1")],
        &[created_event("bytes1")],
    ))
    .unwrap();
    // Independent of the store: raw filesystem read and a separately
    // computed SHA-256 must match the persisted record exactly.
    let raw = fs::read(root.artifacts().join("bytes1")).unwrap();
    assert_eq!(raw, payload);
    assert_eq!(record.content_hash, independent_hash(&payload));
    assert_eq!(record.stored_bytes as usize, payload.len());
}

#[test]
fn capture_marks_truncation_at_the_limit() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let oversized = vec![b'x'; MAX_ARTIFACT_BYTES + 4096];
    let record = rt
        .block_on(store.finalize_artifact(&request("big1", &oversized)))
        .unwrap();
    assert_eq!(record.stored_bytes as usize, MAX_ARTIFACT_BYTES);
    assert_eq!(record.flags, vec![CaptureFlag::Truncated]);
    let raw = fs::read(root.artifacts().join("big1")).unwrap();
    assert_eq!(raw.len(), MAX_ARTIFACT_BYTES);
    assert_eq!(raw, oversized[..MAX_ARTIFACT_BYTES]);
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("big1")],
        &[created_event("big1")],
    ))
    .unwrap();
    assert_eq!(
        rt.block_on(store.verify_artifact("big1")).unwrap(),
        ArtifactStatus::Verified {
            stored_bytes: MAX_ARTIFACT_BYTES as i64,
            content_hash: record.content_hash,
        }
    );
}

#[test]
fn capture_marks_redaction_and_stores_no_secret() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let secret = "sk-super-secret-token";
    let req = CaptureRequest {
        spec: ArtifactSpec {
            provenance: format!("fixture:key={secret}"),
            ..spec("red1")
        },
        content: format!("output with {secret} inside and {secret} twice").into_bytes(),
        secrets: vec![secret.into()],
        omitted: false,
    };
    let record = rt.block_on(store.finalize_artifact(&req)).unwrap();
    assert_eq!(record.flags, vec![CaptureFlag::Redacted]);
    assert!(!record.provenance.contains(secret));
    let raw = fs::read(root.artifacts().join("red1")).unwrap();
    let text = String::from_utf8(raw).unwrap();
    assert!(!text.contains(secret));
    assert!(text.contains("[REDACTED] inside and [REDACTED] twice"));
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("red1")],
        &[created_event("red1")],
    ))
    .unwrap();
    let stored = rt.block_on(store.artifact_record("red1")).unwrap().unwrap();
    assert!(!stored.provenance.contains(secret));
    assert_eq!(
        rt.block_on(store.verify_artifact("red1")).unwrap(),
        ArtifactStatus::Verified {
            stored_bytes: stored.stored_bytes,
            content_hash: stored.content_hash,
        }
    );
}

#[test]
fn omitted_capture_stores_no_content_file() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let req = CaptureRequest {
        spec: spec("om1"),
        content: Vec::new(),
        secrets: Vec::new(),
        omitted: true,
    };
    let record = rt.block_on(store.finalize_artifact(&req)).unwrap();
    assert_eq!(record.flags, vec![CaptureFlag::Omitted]);
    assert_eq!(record.stored_bytes, 0);
    assert!(!root.artifacts().join("om1").exists());
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("om1")],
        &[created_event("om1")],
    ))
    .unwrap();
    assert_eq!(
        rt.block_on(store.verify_artifact("om1")).unwrap(),
        ArtifactStatus::Verified {
            stored_bytes: 0,
            content_hash: String::new(),
        }
    );
    assert_eq!(
        rt.block_on(store.read_artifact_content("om1")).unwrap(),
        Some(Vec::new())
    );
}

#[test]
fn finalized_artifacts_are_immutable() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    rt.block_on(store.finalize_artifact(&request("imm1", b"first")))
        .unwrap();
    // Re-finalizing the same id is refused before any write.
    assert_eq!(
        rt.block_on(store.finalize_artifact(&request("imm1", b"second")))
            .unwrap_err(),
        StoreError::Conflict
    );
    let rec2 = rt
        .block_on(store.finalize_artifact(&request("imm2", b"other")))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(&rec2, &[owner_update("imm2")], &[]))
        .unwrap();
    // Re-capturing a committed id is also refused.
    assert_eq!(
        rt.block_on(store.finalize_artifact(&request("imm2", b"replaced")))
            .unwrap_err(),
        StoreError::Conflict
    );
    assert_eq!(fs::read(root.artifacts().join("imm1")).unwrap(), b"first");
}

#[test]
fn reference_commit_requires_finalized_content() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("fc1", b"content")))
        .unwrap();
    fs::remove_file(root.artifacts().join("fc1")).unwrap();
    // The reference cannot commit without its finalized bytes.
    assert_eq!(
        rt.block_on(store.commit_artifact_reference(
            &record,
            &[owner_update("fc1")],
            &[created_event("fc1")]
        ))
        .unwrap_err(),
        StoreError::Conflict
    );
    assert!(rt.block_on(store.artifact_record("fc1")).unwrap().is_none());
    assert!(rt.block_on(store.state("fc1-owner")).unwrap().is_none());
}

#[test]
fn reference_commit_is_atomic_with_state_and_event() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("at1", b"data")))
        .unwrap();
    // Event referencing a state row that is not part of the unit: the FK
    // violation must discard the artifact reference too.
    let bad_event = EventRecord {
        state_id: "ghost-state".into(),
        kind: "artifact-created".into(),
        payload: "ghost".into(),
    };
    assert_eq!(
        rt.block_on(store.commit_artifact_reference(&record, &[owner_update("at1")], &[bad_event]))
            .unwrap_err(),
        StoreError::Constraint
    );
    assert!(rt.block_on(store.artifact_record("at1")).unwrap().is_none());
    assert!(rt.block_on(store.state("at1-owner")).unwrap().is_none());
    // Content file remains as a detectable orphan, not a dangling reference.
    let orphans = rt.block_on(store.detect_orphaned_artifacts()).unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].name, "at1");
}

#[test]
fn crash_after_content_write_before_reference_commit_leaves_reclaimable_orphan() {
    let root = Root::new();
    if let Some((_, root_path)) = child_mode() {
        let rt = runtime();
        let mut store = rt.block_on(Store::open(&root_path)).unwrap();
        rt.block_on(store.finalize_artifact(&request("orphan1", b"uncommitted")))
            .unwrap();
        fs::write(root_path.join("child-marker"), "finalized").unwrap();
        std::process::abort();
    }
    let rt = runtime();
    let mut child = spawn_child(
        "orphan",
        &root,
        "artifact_tests::crash_after_content_write_before_reference_commit_leaves_reclaimable_orphan",
    );
    wait_for_marker(
        &mut child,
        &root.marker(),
        &root.stderr_log(),
        Duration::from_secs(60),
    );
    let status = child.wait().unwrap();
    assert!(!status.success(), "child must die by abort");
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    // No committed reference exists, but the orphan is detectable.
    assert!(
        rt.block_on(store.artifact_record("orphan1"))
            .unwrap()
            .is_none()
    );
    let orphans = rt.block_on(store.detect_orphaned_artifacts()).unwrap();
    assert_eq!(orphans.len(), 1);
    assert_eq!(orphans[0].name, "orphan1");
    assert_eq!(orphans[0].size, 11);
    // Reclaimable: removing the orphan file leaves a clean scan.
    fs::remove_file(root.artifacts().join("orphan1")).unwrap();
    assert!(
        rt.block_on(store.detect_orphaned_artifacts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn crash_after_reference_commit_keeps_content_verifiable() {
    let root = Root::new();
    if let Some((_, root_path)) = child_mode() {
        let rt = runtime();
        let mut store = rt.block_on(Store::open(&root_path)).unwrap();
        let record = rt
            .block_on(store.finalize_artifact(&request("durable1", b"committed bytes")))
            .unwrap();
        rt.block_on(store.commit_artifact_reference(
            &record,
            &[owner_update("durable1")],
            &[created_event("durable1")],
        ))
        .unwrap();
        fs::write(root_path.join("child-marker"), "committed").unwrap();
        std::process::abort();
    }
    let rt = runtime();
    let mut child = spawn_child(
        "durable",
        &root,
        "artifact_tests::crash_after_reference_commit_keeps_content_verifiable",
    );
    wait_for_marker(
        &mut child,
        &root.marker(),
        &root.stderr_log(),
        Duration::from_secs(60),
    );
    let _ = child.wait().unwrap();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.artifact_record("durable1"))
        .unwrap()
        .expect("committed reference must survive");
    assert_eq!(
        rt.block_on(store.verify_artifact("durable1")).unwrap(),
        ArtifactStatus::Verified {
            stored_bytes: 15,
            content_hash: record.content_hash,
        }
    );
    assert!(
        rt.block_on(store.detect_orphaned_artifacts())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reference_without_content_is_reported_corrupt_not_repaired() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("rwoc", b"present")))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("rwoc")],
        &[created_event("rwoc")],
    ))
    .unwrap();
    fs::remove_file(root.artifacts().join("rwoc")).unwrap();
    assert_eq!(
        rt.block_on(store.verify_artifact("rwoc")).unwrap(),
        ArtifactStatus::Corrupt {
            reason: "content file missing".into()
        }
    );
    // Reading surfaces the corruption instead of returning bad data, and
    // nothing is silently re-created.
    assert_eq!(
        rt.block_on(store.read_artifact_content("rwoc"))
            .unwrap_err(),
        StoreError::Corrupt
    );
    assert!(!root.artifacts().join("rwoc").exists());
}

#[test]
fn torn_content_file_is_detected_and_not_repaired() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("torn1", b"0123456789abcdef")))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("torn1")],
        &[created_event("torn1")],
    ))
    .unwrap();
    let path = root.artifacts().join("torn1");
    let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(7).unwrap(); // simulate a torn write truncating the tail
    drop(file);
    assert_eq!(
        rt.block_on(store.verify_artifact("torn1")).unwrap(),
        ArtifactStatus::Corrupt {
            reason: "stored size mismatch".into()
        }
    );
    // The corrupt bytes stay exactly as found: no silent repair.
    assert_eq!(fs::read(&path).unwrap().len(), 7);
    assert_eq!(
        rt.block_on(store.read_artifact_content("torn1"))
            .unwrap_err(),
        StoreError::Corrupt
    );
}

#[test]
fn symlinked_content_is_never_followed() {
    let root = Root::new();
    let outside = Root::new();
    let outside_file = outside.0.join("victim");
    fs::write(&outside_file, b"outside data").unwrap();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let record = rt
        .block_on(store.finalize_artifact(&request("sym1", b"mine")))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(
        &record,
        &[owner_update("sym1")],
        &[created_event("sym1")],
    ))
    .unwrap();
    fs::remove_file(root.artifacts().join("sym1")).unwrap();
    std::os::unix::fs::symlink(&outside_file, root.artifacts().join("sym1")).unwrap();
    assert_eq!(
        rt.block_on(store.verify_artifact("sym1")).unwrap(),
        ArtifactStatus::Corrupt {
            reason: "content file unreadable (symlink or unsafe)".into()
        }
    );
    assert_eq!(
        rt.block_on(store.read_artifact_content("sym1"))
            .unwrap_err(),
        StoreError::Corrupt
    );
    // The symlink target was never touched.
    assert_eq!(fs::read(&outside_file).unwrap(), b"outside data");
}

#[test]
fn artifact_ids_are_confined_to_the_managed_root() {
    let root = Root::new();
    let outside = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    for bad_id in [
        "../escape",
        "a/b",
        "/abs/path",
        "..",
        ".hidden",
        "",
        "with space",
        "ctrl\x01char",
    ] {
        let mut bad = spec("placeholder");
        bad.id = bad_id.into();
        bad.state_id = "owner-ok".into();
        let req = CaptureRequest {
            spec: bad,
            content: b"payload".to_vec(),
            secrets: Vec::new(),
            omitted: false,
        };
        assert_eq!(
            rt.block_on(store.finalize_artifact(&req)).unwrap_err(),
            StoreError::UnsupportedPath,
            "id {bad_id:?} must be rejected"
        );
    }
    // Nothing was written outside the managed root.
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    // Rejected ids never create the artifacts root or any file in it.
    let artifacts = root.0.join(".fluzodrive").join(ARTIFACTS_DIRECTORY);
    match fs::read_dir(&artifacts) {
        Ok(entries) => assert_eq!(entries.count(), 0),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
}

#[test]
fn orphan_scan_lists_unreferenced_files_only() {
    let root = Root::new();
    let rt = runtime();
    let mut store = rt.block_on(Store::open(&root.0)).unwrap();
    let committed = rt
        .block_on(store.finalize_artifact(&request("kept1", b"committed")))
        .unwrap();
    rt.block_on(store.commit_artifact_reference(
        &committed,
        &[owner_update("kept1")],
        &[created_event("kept1")],
    ))
    .unwrap();
    let uncommitted = rt
        .block_on(store.finalize_artifact(&request("lost1", b"never committed")))
        .unwrap();
    assert!(!uncommitted.flags.contains(&CaptureFlag::Omitted));
    let orphans = rt.block_on(store.detect_orphaned_artifacts()).unwrap();
    let names: Vec<&str> = orphans.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, vec!["lost1"]);
}
