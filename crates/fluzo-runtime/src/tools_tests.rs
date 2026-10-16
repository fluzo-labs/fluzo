//! Tool dispatch tests.
//!
//! The seam under test has one job: make sure nothing runs that policy did not
//! allow, and that what does run is the thing the approval described. Each test
//! therefore checks both the returned value and the presence or absence of the
//! side effect, using a sentinel file that only an actual spawn would create.
//!
//! Fixtures use the real jail and real processes. A mocked spawn would test the
//! wiring rather than the property, and the property is about processes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fluzo_core::policy::{
    Denial, DenialScope, ExecutionBackend, FileOperation, FileScope, Grant, GrantLifetime,
    GrantState, PolicyReason, PolicySubject, ResolvedPermission, ShellMode, ShellScope,
    SubjectKind,
};
use fluzo_core::settings::Permission;

use crate::confinement::{ConfinementError, WorkspaceJail};
use crate::git_inspect::{self, Inspection};
use crate::process::{self, EnvironmentSpec, ProcessRequest, SpawnError, TerminationMode};
use crate::session_auth::{ApprovalMode, AuthorizationService};
use crate::tools::{
    CommandTool, ConstantPermission, ExecutionResult, Limits, ToolDispatcher, ToolError,
    ToolOutcome, ToolRequest,
};

const WS: &str = "3f2a9c14be07d5a8";

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);

impl Root {
    fn join(&self, suffix: &str) -> PathBuf {
        self.0.join(suffix)
    }

    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-tools-{tag}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ask_permission() -> ResolvedPermission {
    ResolvedPermission {
        value: Permission::Ask,
        origin: fluzo_core::policy::PermissionOrigin::BuiltIn,
        ignored_repository_broaden: None,
    }
}

fn grant(id: &str, subject: PolicySubject) -> Grant {
    Grant {
        id: id.to_owned(),
        subject,
        workspace: WS.to_owned(),
        backend: ExecutionBackend::NativeLocal,
        lifetime: GrantLifetime::OneTime,
        state: GrantState::Issued,
    }
}

/// A `touch <sentinel>` command whose approved scope matches what it runs.
fn touch_command(root: &Path, sentinel: &str) -> (CommandTool, PathBuf) {
    let sentinel_path = root.join(sentinel);
    let arguments = vec![sentinel_path.to_str().expect("utf-8 sentinel").to_string()];
    let scope = ShellScope {
        mode: ShellMode::Direct,
        executable: "/bin/touch".to_string(),
        arguments: arguments.clone(),
        working_directory: ".".to_string(),
        environment: BTreeMap::new(),
    };
    let tool = CommandTool {
        dispatch_id: "d1".to_string(),
        approval_id: "a1".to_string(),
        working_directory: ".".to_string(),
        approved: scope,
        request: ProcessRequest::direct("/bin/touch", arguments),
        environment: EnvironmentSpec::empty(),
        limits: Limits::default(),
    };
    (tool, sentinel_path)
}

fn read_request(path: &str) -> ToolRequest {
    ToolRequest::ReadFile {
        dispatch_id: "d1".to_string(),
        approval_id: "a1".to_string(),
        path: path.to_string(),
    }
}

fn file_subject(operation: FileOperation, path: &str) -> PolicySubject {
    PolicySubject::Filesystem(FileScope::new(operation, path))
}

// --- denial wins ---------------------------------------------------------

#[test]
fn a_denied_command_never_reaches_spawn() {
    let root = Root::new("deny");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");
    auth.ledger_mut()
        .add_denial(Denial {
            id: "d-shell".to_string(),
            scope: DenialScope::Kind(SubjectKind::Shell),
        })
        .expect("denial added");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("a refusal is an outcome, not an error");

    match outcome {
        ToolOutcome::Denied {
            reason: PolicyReason::DeniedByUser,
        } => {}
        other => panic!("expected a user denial, got {other:?}"),
    }
    assert!(
        !sentinel.exists(),
        "a denied request must not have spawned anything"
    );
}

#[test]
fn a_denial_beats_a_live_grant_and_spends_neither() {
    let root = Root::new("deny-over-grant");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");
    let subject = PolicySubject::Shell(tool.approved.clone());
    auth.ledger_mut()
        .issue(grant("g1", subject.clone()))
        .expect("grant issued");
    auth.ledger_mut()
        .add_denial(Denial {
            id: "d-exact".to_string(),
            scope: DenialScope::Exact(Box::new(subject)),
        })
        .expect("denial added");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("dispatch returns an outcome");
    assert!(
        matches!(outcome, ToolOutcome::Denied { .. }),
        "a denial outranks a matching grant: {outcome:?}"
    );
    assert!(!sentinel.exists(), "nothing may run against a denial");
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| !g.is_consumed()),
        "a refused dispatch must not spend the grant either"
    );
}

// --- headless hosts ------------------------------------------------------

#[test]
fn a_headless_host_gets_an_unavailable_outcome_without_blocking() {
    let root = Root::new("headless");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let started = std::time::Instant::now();
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("dispatch returns an outcome");
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, ToolOutcome::ApprovalUnavailable(_)),
        "headless must report the need rather than wait: {outcome:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the headless path must not wait for input, took {elapsed:?}"
    );
    assert!(!sentinel.exists());
}

// --- one grant per dispatch ----------------------------------------------

#[test]
fn exactly_one_grant_is_consumed_per_dispatch() {
    let root = Root::new("one-grant");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");
    let subject = PolicySubject::Shell(tool.approved.clone());
    auth.ledger_mut()
        .issue(grant("g1", subject))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("granted dispatch runs");
    assert!(outcome.executed(), "the granted dispatch must run");
    assert!(sentinel.exists(), "the tool must have actually run");
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| g.state
            == GrantState::Consumed {
                dispatch_id: "d1".to_string()
            }),
        "the grant must be consumed by this dispatch id"
    );
}

#[test]
fn a_second_dispatch_of_the_same_grant_is_refused() {
    let root = Root::new("grant-replay");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");
    let subject = PolicySubject::Shell(tool.approved.clone());
    auth.ledger_mut()
        .issue(grant("g1", subject))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool.clone())))
        .expect("first dispatch runs");
    std::fs::remove_file(&sentinel).expect("sentinel removed for the second attempt");

    let second = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("the second attempt is a refusal, not an error");
    assert!(
        matches!(second, ToolOutcome::ApprovalUnavailable(_)),
        "a spent grant must not authorise a second run: {second:?}"
    );
    assert!(
        !sentinel.exists(),
        "the replayed dispatch must not have run the tool again"
    );
}

// --- approval covers what runs -------------------------------------------

#[test]
fn an_approval_for_different_arguments_is_refused_before_anything_runs() {
    let root = Root::new("mismatch");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (mut tool, sentinel) = touch_command(&root.0, "SENTINEL");

    // The approved scope names one sentinel; the request touches another.
    tool.approved = ShellScope {
        mode: ShellMode::Direct,
        executable: "/bin/touch".to_string(),
        arguments: vec![root.join("OTHER").to_str().unwrap().to_string()],
        working_directory: ".".to_string(),
        environment: BTreeMap::new(),
    };
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(tool.approved.clone())))
        .expect("grant issued for the *approved* scope");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect_err("a mismatched approval must be refused");
    assert!(
        matches!(error, ToolError::SubjectMismatch { .. }),
        "expected a mismatch refusal, got {error:?}"
    );
    assert!(!sentinel.exists(), "a mismatch must not spawn anything");
    assert!(
        !root.join("OTHER").exists(),
        "nor may the approved-but-unrequested path appear"
    );
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| !g.is_consumed()),
        "a refused dispatch must not spend the grant"
    );
}

#[test]
fn an_undeclared_environment_override_is_refused() {
    let root = Root::new("env-override");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (mut tool, sentinel) = touch_command(&root.0, "SENTINEL");

    // The request carries an override the approved scope never mentioned.
    let mut environment = EnvironmentSpec::empty();
    environment
        .inject("SOMETHING_EXTRA", "value")
        .expect("inject allowed");
    tool.environment = environment;
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(tool.approved.clone())))
        .expect("grant issued for the unmodified scope");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect_err("an undisclosed override must be refused");
    assert!(
        matches!(error, ToolError::SubjectMismatch { .. }),
        "expected a mismatch refusal, got {error:?}"
    );
    assert!(!sentinel.exists());
}

#[test]
fn a_declared_environment_override_matches_and_runs() {
    let root = Root::new("env-declared");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let (mut tool, sentinel) = touch_command(&root.0, "SENTINEL");

    let mut environment = EnvironmentSpec::empty();
    environment
        .inject("FLUZO_TEST_VAR", "value")
        .expect("inject allowed");
    tool.approved.environment = environment.overrides();
    tool.environment = environment;
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(tool.approved.clone())))
        .expect("grant issued for the declared scope");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("a declared override is not a mismatch");
    assert!(outcome.executed());
    assert!(sentinel.exists());
}

// --- no shell fallback ---------------------------------------------------

#[test]
fn a_failed_direct_spawn_does_not_become_a_shell_attempt() {
    let root = Root::new("no-fallback");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    let missing = "/bin/definitely-not-installed-binary";
    let arguments = vec![root.join("ARG").to_str().unwrap().to_string()];
    let scope = ShellScope {
        mode: ShellMode::Direct,
        executable: missing.to_string(),
        arguments: arguments.clone(),
        working_directory: ".".to_string(),
        environment: BTreeMap::new(),
    };
    let tool = CommandTool {
        dispatch_id: "d1".to_string(),
        approval_id: "a1".to_string(),
        working_directory: ".".to_string(),
        approved: scope.clone(),
        request: ProcessRequest::direct(missing, arguments),
        environment: EnvironmentSpec::empty(),
        limits: Limits::default(),
    };
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(scope)))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect_err("a missing executable must fail");
    assert!(
        matches!(
            error,
            ToolError::Spawn(SpawnError::ExecutableNotFound { .. })
        ),
        "the failure must be the spawn failure itself, got {error:?}"
    );
    assert!(
        !root.join("SHELL_RAN").exists() && !root.join("ARG").exists(),
        "no shell may be tried after a native refusal"
    );
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| g.is_consumed()),
        "the grant was spent by the dispatch that was actually attempted"
    );
}

// --- uncertain outcomes --------------------------------------------------

#[test]
fn an_unconfirmed_termination_marks_the_grant_uncertain_and_keeps_it_spent() {
    let root = Root::new("uncertain");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);

    // A sleep that cannot exit inside a zero-length timeout, so the dispatcher
    // reaches the uncertain branch deterministically.
    let scope = ShellScope {
        mode: ShellMode::Direct,
        executable: "/bin/sleep".to_string(),
        arguments: vec!["30".to_string()],
        working_directory: ".".to_string(),
        environment: BTreeMap::new(),
    };
    let tool = CommandTool {
        dispatch_id: "d1".to_string(),
        approval_id: "a1".to_string(),
        working_directory: ".".to_string(),
        approved: scope.clone(),
        request: ProcessRequest::direct("/bin/sleep", vec!["30".to_string()]),
        environment: EnvironmentSpec::empty(),
        limits: Limits {
            capture_limit: 4096,
            timeout: Duration::ZERO,
            grace: Duration::ZERO,
            escalation: Duration::ZERO,
        },
    };
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(scope)))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("the dispatch runs and is cancelled");
    match outcome {
        ToolOutcome::Executed(ExecutionResult::Process(process)) => {
            assert_eq!(
                process.termination,
                TerminationMode::Uncertain,
                "a zero-window cancellation must report uncertainty"
            );
        }
        other => panic!("expected an executed process, got {other:?}"),
    }
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| g.is_consumed()),
        "uncertainty never re-arms a grant"
    );
    assert!(
        auth.ledger().is_uncertain("g1"),
        "the unresolved outcome must be visible"
    );
}

// --- jail --------------------------------------------------------------

#[test]
fn a_file_tool_cannot_reach_a_path_the_jail_refuses() {
    let root = Root::new("jail-refusal");
    std::fs::create_dir(root.join(".git")).expect("protected dir");
    std::fs::write(root.join(".git/config"), "[core]\n").expect("protected file");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    auth.ledger_mut()
        .issue(grant(
            "g1",
            file_subject(FileOperation::Read, ".git/config"),
        ))
        .expect("a grant for exactly this read is issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&read_request(".git/config"))
        .expect_err("the jail must refuse a protected name");
    assert!(
        matches!(
            error,
            ToolError::Confinement(ConfinementError::Protected { .. })
        ),
        "expected a protected-name refusal, got {error:?}"
    );
}

#[test]
fn a_jail_refusal_is_returned_rather_than_retried_as_a_command() {
    let root = Root::new("jail-no-retry");
    std::fs::create_dir(root.join(".fluzodrive")).expect("protected dir");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    auth.ledger_mut()
        .issue(grant(
            "g1",
            file_subject(FileOperation::Read, ".fluzodrive/state.sqlite3"),
        ))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&read_request(".fluzodrive/state.sqlite3"))
        .expect_err("the jail refuses");
    // The error is the jail's own, not a spawn or shell error: the refusal was
    // returned rather than converted into another attempt.
    assert!(matches!(error, ToolError::Confinement(_)));
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| g.is_consumed()),
        "the grant was spent by the attempt that was actually made"
    );
}

#[test]
fn an_allowed_file_read_goes_through_the_jail() {
    let root = Root::new("jail-read");
    std::fs::write(root.join("notes.txt"), "confined contents").expect("write fixture");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    auth.ledger_mut()
        .issue(grant("g1", file_subject(FileOperation::Read, "notes.txt")))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&read_request("notes.txt"))
        .expect("the read is allowed and confined");
    match outcome {
        ToolOutcome::Executed(ExecutionResult::Contents(text)) => {
            assert_eq!(text, "confined contents");
        }
        other => panic!("expected confined contents, got {other:?}"),
    }
}

#[test]
fn an_allowed_file_write_lands_inside_the_jail() {
    let root = Root::new("jail-write");
    std::fs::write(root.join("target.txt"), "before").expect("fixture");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Interactive);
    auth.ledger_mut()
        .issue(grant(
            "g1",
            file_subject(FileOperation::Write, "target.txt"),
        ))
        .expect("grant issued");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::WriteFile {
            dispatch_id: "d1".to_string(),
            approval_id: "a1".to_string(),
            path: "target.txt".to_string(),
            contents: b"after".to_vec(),
        })
        .expect("the write is allowed and confined");
    assert!(matches!(
        outcome,
        ToolOutcome::Executed(ExecutionResult::Written { bytes: 5, .. })
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("target.txt")).expect("read back"),
        "after"
    );
}

// --- YOLO --------------------------------------------------------------

#[test]
fn yolo_suppresses_a_prompt_but_not_a_denial() {
    let root = Root::new("yolo-denial");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    auth.set_yolo(true);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");
    auth.ledger_mut()
        .add_denial(Denial {
            id: "d-shell".to_string(),
            scope: DenialScope::Kind(SubjectKind::Shell),
        })
        .expect("denial added");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("YOLO does not turn a denial into an error");
    assert!(
        matches!(outcome, ToolOutcome::Denied { .. }),
        "YOLO must not suppress a denial: {outcome:?}"
    );
    assert!(!sentinel.exists());
}

#[test]
fn yolo_suppresses_a_prompt_but_not_a_jail_refusal() {
    let root = Root::new("yolo-jail");
    std::fs::create_dir(root.join(".git")).expect("protected dir");
    std::fs::write(root.join(".git/config"), "[core]\n").expect("protected file");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    auth.set_yolo(true);

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&read_request(".git/config"))
        .expect_err("YOLO must not bypass the jail");
    assert!(matches!(error, ToolError::Confinement(_)));
}

#[test]
fn yolo_allows_an_ungranted_command_and_it_runs() {
    let root = Root::new("yolo-runs");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    auth.set_yolo(true);
    let (tool, sentinel) = touch_command(&root.0, "SENTINEL");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::Command(Box::new(tool)))
        .expect("YOLO converts the missing approval into a proceed");
    assert!(outcome.executed(), "YOLO must let the operation proceed");
    assert!(sentinel.exists());
}

// --- git inspection ----------------------------------------------------

#[test]
fn a_git_inspection_runs_through_the_same_policy_and_process_path() {
    let root = Root::new("git-inspect");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);

    let arguments = git_inspect::inspection_arguments(&Inspection::Status).expect("arguments");
    let environment = git_inspect::hardened_git_environment();
    let scope = ShellScope {
        mode: ShellMode::Direct,
        executable: "git".to_string(),
        arguments,
        working_directory: ".".to_string(),
        environment: environment.overrides(),
    };
    auth.ledger_mut()
        .issue(grant("g1", PolicySubject::Shell(scope)))
        .expect("grant issued for the exact inspection argv");

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::InspectGit {
            dispatch_id: "d1".to_string(),
            approval_id: "a1".to_string(),
            inspection: Inspection::Status,
        })
        .expect("the inspection dispatches");
    assert!(outcome.executed());
    assert!(
        auth.ledger().grant("g1").is_some_and(|g| g.is_consumed()),
        "the inspection consumed its grant"
    );
}

#[test]
fn an_inspection_outside_the_closed_set_never_reaches_the_process_layer() {
    let root = Root::new("inspect-closed");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    // A grant for an arbitrary `git push` argv is not an inspection.
    let request = ToolRequest::InspectGit {
        dispatch_id: "d1".to_string(),
        approval_id: "a1".to_string(),
        inspection: Inspection::Show {
            rev: "--help; rm -rf /".to_string(),
        },
    };
    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let error = dispatcher
        .dispatch(&request)
        .expect_err("a rejected inspection must not dispatch");
    assert!(matches!(error, ToolError::Inspection(_)), "got {error:?}");
}

#[test]
fn an_inspection_without_a_grant_is_refused_in_a_headless_host() {
    let root = Root::new("inspect-no-grant");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);

    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let outcome = dispatcher
        .dispatch(&ToolRequest::InspectGit {
            dispatch_id: "d1".to_string(),
            approval_id: "a1".to_string(),
            inspection: Inspection::Status,
        })
        .expect("the refusal is an outcome");
    assert!(
        matches!(outcome, ToolOutcome::ApprovalUnavailable(_)),
        "an ungranted inspection must not run headless: {outcome:?}"
    );
}

// --- process module re-exports used by the seam --------------------------

#[test]
fn the_seam_exposes_the_process_layer_only_through_its_own_types() {
    // A compile-level guard: the dispatcher takes a SpawnConfig built by the
    // caller's request, so a caller cannot hand it a raw pid or a pre-opened
    // child handle.
    let root = Root::new("types");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");
    let mut auth = AuthorizationService::new(ApprovalMode::Headless);
    auth.set_yolo(true);
    let (tool, _sentinel) = touch_command(&root.0, "SENTINEL");
    let permission = ConstantPermission(ask_permission());
    let mut dispatcher = ToolDispatcher::new(&mut auth, &jail, &root.0, WS, &permission);
    let request = ToolRequest::Command(Box::new(tool));
    let _ = process::DEFAULT_CAPTURE_LIMIT;
    assert!(dispatcher.dispatch(&request).is_ok());
}
