//! RUN-01a: workspace identity and cross-process writer ownership.
//!
//! The contract is about two processes, so the fixtures use two
//! processes. A single-process test can only prove that a lock call
//! returns what it returns; it cannot prove that another Fluzo runtime
//! is excluded. Child roles run by re-invoking this test binary with
//! `--ignored --exact`, and synchronization is a pipe: the child
//! announces a state on stdout and the parent acts on that, never on a
//! guessed delay.
//!
//! Fixture roots follow the #81/#94 discipline: a canonicalized temp
//! root, explicit 0700, and a failed child reported with its captured
//! stderr so a runner flake is distinguishable from a defect.

use crate::store::{DRIVE_DIRECTORY, EventRecord, StateUpdate, Store};
use crate::workspace::*;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::runtime::{Builder, Runtime};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
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

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-own-{tag}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn lock_path(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY).join(WRITER_LOCK_FILE)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A child running one of the holder roles below, plus the owner nonce
/// it announced at startup.
struct Holder {
    child: Child,
    reader: BufReader<ChildStdout>,
    stdin: ChildStdin,
    owner: String,
}

impl Holder {
    /// Spawns a child in `mode` and returns once it has announced
    /// `READY <owner>`. That announcement is the barrier. Lines that
    /// are not ours (the libtest harness preamble the child itself
    /// emits) are skipped rather than mistaken for the announcement.
    fn start(mode: &str, root: &Path) -> Self {
        let exe = std::env::current_exe().expect("current exe");
        let mut child = Command::new(exe)
            .arg("--ignored")
            .arg("--exact")
            .arg("workspace::tests::child_role")
            .arg("--nocapture")
            .env("FLUZO_TEST_ROLE", mode)
            .env("FLUZO_TEST_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn child holder");
        let stdout = child.stdout.take().expect("child stdout");
        let stdin = child.stdin.take().expect("child stdin");
        let mut reader = BufReader::new(stdout);
        let line = read_announcement(&mut reader, &mut child, "READY ");
        let owner = line["READY ".len()..].trim().to_owned();
        Self {
            child,
            reader,
            stdin,
            owner,
        }
    }

    fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").expect("write to child stdin");
        self.stdin.flush().expect("flush child stdin");
    }

    /// Sends one command and asserts the announced reply.
    fn exchange(&mut self, command: &str, expected: &str) -> String {
        self.send(command);
        let line = read_announcement(&mut self.reader, &mut self.child, expected);
        assert_eq!(
            line.trim(),
            expected,
            "child reply to {command} was something else"
        );
        line
    }

    fn read_line(&mut self, prefix: &str) -> String {
        read_announcement(&mut self.reader, &mut self.child, prefix)
    }

    fn wait_finished(&mut self) -> std::process::ExitStatus {
        self.child.wait().expect("wait for child")
    }
}

/// Reads until a line beginning with `prefix` arrives, then returns it.
/// If the child dies first, the captured stderr is included: a child
/// panic otherwise surfaces as a truncated read and cannot be
/// diagnosed from a CI log.
fn read_announcement(
    reader: &mut BufReader<ChildStdout>,
    child: &mut Child,
    prefix: &str,
) -> String {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let status = child.wait().expect("wait after early exit");
                let captured = child
                    .stderr
                    .take()
                    .map(|pipe| {
                        let mut text = String::new();
                        let _ = BufReader::new(pipe).read_to_string(&mut text);
                        text
                    })
                    .unwrap_or_default();
                panic!("child exited {status} before announcing {prefix:?}; stderr:\n{captured}");
            }
            Ok(_) => {
                if line.starts_with(prefix) {
                    return line;
                }
            }
            Err(error) => panic!("failed reading child output: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Child roles
// ---------------------------------------------------------------------------

/// Entry point for the child processes spawned above. Marked `ignore`
/// so it never runs as a normal test; the fixtures invoke it directly.
#[test]
#[ignore = "child role driven by the ownership fixtures, not run standalone"]
fn child_role() {
    let role = std::env::var("FLUZO_TEST_ROLE").expect("FLUZO_TEST_ROLE");
    let root = PathBuf::from(std::env::var("FLUZO_TEST_ROOT").expect("FLUZO_TEST_ROOT"));
    let ownership = WriterOwnership::acquire(&root)
        .unwrap_or_else(|error| panic!("child could not acquire {root:?}: {error}"));
    match role.as_str() {
        "hold" => {
            announce(&format!("READY {}", ownership.record().owner));
            wait_for_command();
            let receipt = ownership
                .release()
                .unwrap_or_else(|refusal| panic!("child release refused: {refusal}"));
            announce(&format!("RELEASED {}", receipt.owner));
        }
        "hold_and_commit" => {
            announce(&format!("READY {}", ownership.record().owner));
            wait_for_command();
            // Unrelated durable work while ownership is held. The store
            // is opened by the owner itself: the point is that the
            // SQLite write lock comes and goes underneath a held
            // ownership without disturbing it.
            let runtime = runtime();
            let mut store = runtime
                .block_on(Store::open(&root))
                .expect("child opens the drive");
            for index in 0..3 {
                let state_id = format!("holder-{index}");
                runtime
                    .block_on(store.commit_unit(
                        &[update(&state_id, &index.to_string())],
                        &[event(&state_id, "created")],
                    ))
                    .expect("child commit");
                runtime
                    .block_on(store.commit_unit(
                        &[update(&state_id, &(index + 10).to_string())],
                        &[event(&state_id, "updated")],
                    ))
                    .expect("child update");
            }
            runtime
                .block_on(store.checkpoint_for_test())
                .expect("child checkpoint");
            announce("COMMITTED");
            wait_for_command();
            let _ = ownership.release();
        }
        "crash" => {
            announce(&format!("READY {}", ownership.record().owner));
            // Leave without releasing. The kernel drops the lock with
            // the process; the record stays un-released, which is what
            // a crash looks like to the next acquirer.
            std::mem::forget(ownership);
            std::process::exit(9);
        }
        other => panic!("unknown child role {other}"),
    }
}

fn announce(text: &str) {
    let stdout = std::io::stdout();
    let _guard = stdout.lock();
    println!("{text}");
}

/// Blocks until one line arrives on stdin. The child never polls a
/// clock; the parent's write is the event.
fn wait_for_command() {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("read command from parent");
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

#[test]
fn identity_comes_from_the_filesystem_not_the_path_string() {
    let root = Root::new("identity");
    let direct = WorkspaceIdentity::resolve(&root.0).expect("resolve direct");
    // A second name for the same directory must agree, because identity
    // is the directory, not the spelling.
    let link = root.0.join("alias");
    std::os::unix::fs::symlink(&root.0, &link).unwrap();
    let via_link = WorkspaceIdentity::resolve(&link).expect("resolve through symlink");
    assert_eq!(direct, via_link, "two names for one directory disagreed");
    assert_eq!(direct.dev(), via_link.dev());
    assert_eq!(direct.ino(), via_link.ino());
}

#[test]
fn identity_fingerprint_does_not_leak_the_path() {
    let root = Root::new("fingerprint");
    let identity = WorkspaceIdentity::resolve(&root.0).expect("resolve");
    let fingerprint = identity.fingerprint();
    assert_eq!(fingerprint.len(), 16, "fingerprint shape changed");
    let path = root.0.to_string_lossy();
    assert!(
        !path.contains(&fingerprint),
        "fingerprint must not be derived from the readable path: {fingerprint} vs {path}"
    );
    // Stable across calls, so it can key a trace.
    assert_eq!(fingerprint, identity.fingerprint());
}

#[test]
fn different_workspaces_have_different_identities() {
    let first = Root::new("ident-a");
    let second = Root::new("ident-b");
    let a = WorkspaceIdentity::resolve(&first.0).expect("resolve a");
    let b = WorkspaceIdentity::resolve(&second.0).expect("resolve b");
    assert_ne!(a, b);
    assert_ne!(a.fingerprint(), b.fingerprint());
}

#[test]
fn a_missing_root_is_refused_rather_than_created() {
    let missing = std::env::temp_dir().join(format!(
        "fluzo-own-missing-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let error =
        WorkspaceIdentity::resolve(&missing).expect_err("resolving a missing root must fail");
    assert_eq!(error, OwnershipError::Inaccessible);
    assert!(
        !missing.exists(),
        "a failed resolve must not create anything"
    );
}

#[test]
fn a_path_that_cannot_be_resolved_is_refused_without_being_created() {
    // A relative path cannot be resolved to a working-tree root. The
    // contract refuses rather than guessing a base directory, because
    // guessing is how a writer ends up in the wrong workspace.
    let error = WorkspaceIdentity::resolve(Path::new("relative-workspace-that-does-not-exist"))
        .expect_err("a relative root must not resolve");
    assert_eq!(error, OwnershipError::Inaccessible);
}

// ---------------------------------------------------------------------------
// Mutual exclusion across processes
// ---------------------------------------------------------------------------

#[test]
fn a_second_runtime_cannot_write_a_workspace_that_is_already_owned() {
    let root = Root::new("exclude");
    let mut holder = Holder::start("hold", &root.0);
    match WriterOwnership::acquire(&root.0) {
        Err(OwnershipError::HeldBy { holder: record }) => {
            assert_eq!(
                record.owner, holder.owner,
                "refusal must name the holder that is actually running"
            );
            assert_ne!(
                record.pid,
                std::process::id(),
                "the named holder must be the other process"
            );
            assert_eq!(
                record.workspace,
                WorkspaceIdentity::resolve(&root.0)
                    .expect("resolve")
                    .fingerprint(),
                "the refusal must name this workspace"
            );
        }
        other => panic!("expected a named-holder refusal, got {other:?}"),
    }
    holder.send("STOP");
    let _ = holder.wait_finished();
}

#[test]
fn a_refused_acquire_writes_nothing() {
    let root = Root::new("refusal-inert");
    let runtime = runtime();
    runtime.block_on(Store::open(&root.0)).expect("drive");
    assert_eq!(
        lock_bytes(&root),
        None,
        "no lock file before anyone acquires"
    );
    let mut holder = Holder::start("hold", &root.0);
    let observed = lock_bytes(&root).expect("the holder wrote its record");
    match WriterOwnership::acquire(&root.0) {
        Err(OwnershipError::HeldBy { .. }) => {}
        other => panic!("expected refusal, got {other:?}"),
    }
    assert_eq!(
        lock_bytes(&root),
        Some(observed),
        "a refused acquire must leave the record byte-identical"
    );
    holder.send("STOP");
    let _ = holder.wait_finished();
}

#[test]
fn ownership_survives_the_store_write_lock_coming_and_going() {
    let root = Root::new("survive-commits");
    let mut holder = Holder::start("hold_and_commit", &root.0);
    // Refused before the commits.
    assert!(matches!(
        WriterOwnership::acquire(&root.0),
        Err(OwnershipError::HeldBy { .. })
    ));
    holder.exchange("COMMIT", "COMMITTED");
    // And refused after them. The SQLite write lock was taken and
    // released six times in between, which is exactly why it cannot be
    // the thing that owns a workspace.
    match WriterOwnership::acquire(&root.0) {
        Err(OwnershipError::HeldBy { holder: record }) => {
            assert_eq!(
                record.owner, holder.owner,
                "ownership changed across commits"
            );
        }
        other => panic!("expected the same holder after commits, got {other:?}"),
    }
    holder.send("STOP");
    let _ = holder.wait_finished();
}

#[test]
fn a_killed_owner_is_reconciled_and_recorded_as_such() {
    let root = Root::new("crash");
    let mut holder = Holder::start("crash", &root.0);
    let dead_owner = holder.owner.clone();
    let status = holder.wait_finished();
    assert!(!status.success(), "the crash role must not exit cleanly");

    // Before anyone takes it, the abandoned record is visible.
    match WriterOwnership::probe(&root.0).expect("probe after crash") {
        OwnerProbe::Abandoned(record) => assert_eq!(record.owner, dead_owner),
        other => panic!("expected an abandoned record, got {other:?}"),
    }

    // Taking it is a reconciliation, and says so.
    let ownership = WriterOwnership::acquire(&root.0).expect("acquire after crash");
    assert_eq!(
        ownership.record().reconciled_from.as_deref(),
        Some(dead_owner.as_str()),
        "a dead holder must be reconciled, not silently replaced"
    );
    assert_ne!(ownership.record().owner, dead_owner);
    ownership.release().expect("release");
}

#[test]
fn a_clean_release_is_a_handoff_not_a_reconciliation() {
    let root = Root::new("clean-handoff");
    let mut holder = Holder::start("hold", &root.0);
    let released_owner = holder.owner.clone();
    holder.send("STOP");
    let reply = holder.read_line("RELEASED ");
    assert_eq!(
        reply.trim(),
        format!("RELEASED {released_owner}"),
        "expected a clean release of {released_owner}"
    );
    let _ = holder.wait_finished();

    match WriterOwnership::probe(&root.0).expect("probe after release") {
        OwnerProbe::Released(record) => assert_eq!(record.owner, released_owner),
        other => panic!("expected a released record, got {other:?}"),
    }

    let ownership = WriterOwnership::acquire(&root.0).expect("acquire after release");
    assert_eq!(
        ownership.record().reconciled_from,
        None,
        "a cleanly released workspace must not be reported as reconciled"
    );
    ownership.release().expect("release");
}

#[test]
fn probing_a_workspace_with_no_drive_reports_no_drive() {
    let root = Root::new("no-drive");
    let error = WriterOwnership::probe(&root.0)
        .expect_err("probing a workspace with no drive reports no drive");
    assert_eq!(error, OwnershipError::Inaccessible);
}

#[test]
fn an_unowned_drive_probes_free() {
    let root = Root::new("free-drive");
    let runtime = runtime();
    runtime.block_on(Store::open(&root.0)).expect("open drive");
    assert_eq!(
        WriterOwnership::probe(&root.0).expect("probe"),
        OwnerProbe::Free
    );
}

// ---------------------------------------------------------------------------
// Quiescent release
// ---------------------------------------------------------------------------

#[test]
fn release_is_refused_while_a_mutation_is_outstanding_and_the_lock_stays_held() {
    let root = Root::new("not-quiescent");
    let ownership = WriterOwnership::acquire(&root.0).expect("acquire");
    let ticket = ownership.begin_mutation();
    assert_eq!(ownership.outstanding(), 1);
    let refusal = ownership.release().expect_err("release must refuse");
    assert_eq!(refusal.outstanding, 1);
    assert_eq!(refusal.uncertain, 0);
    // The refusal returned the ownership, so the workspace is still
    // owned and a second process is still excluded.
    let ownership = refusal.ownership;
    assert!(matches!(
        WriterOwnership::acquire(&root.0),
        Err(OwnershipError::HeldBy { .. })
    ));
    ticket.completed();
    assert_eq!(ownership.outstanding(), 0);
    ownership.release().expect("release once quiescent");
}

#[test]
fn an_uncertain_writer_keeps_release_refused_after_the_ticket_is_gone() {
    let root = Root::new("uncertain");
    let ownership = WriterOwnership::acquire(&root.0).expect("acquire");
    let ticket = ownership.begin_mutation();
    ticket.uncertain();
    assert_eq!(ownership.outstanding(), 0, "the ticket slot was released");
    assert_eq!(
        ownership.uncertain(),
        1,
        "uncertainty must survive the ticket"
    );
    let refusal = ownership
        .release()
        .expect_err("uncertain writer blocks release");
    assert_eq!(refusal.uncertain, 1);
    assert!(matches!(
        WriterOwnership::acquire(&root.0),
        Err(OwnershipError::HeldBy { .. })
    ));
    // Still refused later: uncertainty does not decay on its own.
    let refusal = refusal.ownership.release().expect_err("still refused");
    assert_eq!(refusal.uncertain, 1);
    refusal
        .ownership
        .release()
        .expect_err("refused a third time")
        .ownership
        .release()
        .expect_err("and a fourth");
}

#[test]
fn release_writes_a_durable_marker_before_giving_up_the_lock() {
    let root = Root::new("durable-marker");
    let ownership = WriterOwnership::acquire(&root.0).expect("acquire");
    let owner = ownership.record().owner.clone();
    let receipt = ownership.release().expect("release");
    assert_eq!(receipt.owner, owner);
    let text = fs::read_to_string(root.lock_path()).expect("lock file readable");
    assert!(
        text.contains(&format!("owner={owner}")) && text.contains("released_ms="),
        "the released marker must be on disk: {text}"
    );
    assert!(
        !text.contains("released_ms=none"),
        "the marker must carry a timestamp, not a placeholder"
    );
}

// ---------------------------------------------------------------------------
// Lock-file integrity
// ---------------------------------------------------------------------------

#[test]
fn a_symlinked_lock_path_is_refused_not_followed() {
    let root = Root::new("lock-symlink");
    let outside = Root::new("lock-target");
    let drive = root.0.join(DRIVE_DIRECTORY);
    fs::create_dir_all(&drive).unwrap();
    std::os::unix::fs::symlink(outside.lock_path(), root.lock_path()).unwrap();
    let error =
        WriterOwnership::acquire(&root.0).expect_err("a symlinked lock path must be refused");
    assert_eq!(error, OwnershipError::Conflict);
    assert!(
        !outside.lock_path().exists(),
        "the refusal must not have created anything at the link target"
    );
}

#[test]
fn a_record_naming_another_workspace_is_refused() {
    let root = Root::new("foreign-record");
    let runtime = runtime();
    runtime.block_on(Store::open(&root.0)).expect("drive");
    fs::write(
        root.lock_path(),
        "fluzo-writer-lock v1\npid=4242\nacquired_ms=1\nworkspace=deadbeefdeadbeef\nowner=foreignowner00000\nreconciled_from=none\nreleased_ms=none\n",
    )
    .unwrap();
    let error =
        WriterOwnership::acquire(&root.0).expect_err("a foreign workspace record must be refused");
    match error {
        OwnershipError::CorruptRecord { reason } => {
            assert!(
                reason.contains("deadbeefdeadbeef"),
                "the refusal must name the foreign workspace: {reason}"
            );
        }
        other => panic!("expected a corrupt-record refusal, got {other:?}"),
    }
}

#[test]
fn a_malformed_record_is_reported_as_malformed() {
    let root = Root::new("malformed");
    let runtime = runtime();
    runtime.block_on(Store::open(&root.0)).expect("drive");
    fs::write(root.lock_path(), "this is not a lock record\n").unwrap();
    let error = WriterOwnership::acquire(&root.0).expect_err("a malformed record must be refused");
    assert!(
        matches!(error, OwnershipError::CorruptRecord { .. }),
        "expected corrupt-record, got {error:?}"
    );
}

#[test]
fn verify_still_owned_detects_the_drive_being_swapped() {
    let root = Root::new("swap-drive");
    let runtime = runtime();
    runtime.block_on(Store::open(&root.0)).expect("drive");
    let ownership = WriterOwnership::acquire(&root.0).expect("acquire");
    ownership
        .verify_still_owned()
        .expect("an unchanged drive verifies");
    // Replace the drive directory with a different one.
    let original = root.0.join(DRIVE_DIRECTORY);
    let moved = root.0.join("drive-moved");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    let error = ownership
        .verify_still_owned()
        .expect_err("a replaced drive must be detected");
    assert_eq!(error, OwnershipError::Conflict);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn lock_bytes(root: &Root) -> Option<Vec<u8>> {
    fs::read(root.lock_path()).ok()
}
