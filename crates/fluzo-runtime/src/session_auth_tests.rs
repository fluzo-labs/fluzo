use std::io::Read;
use std::process::{Command, Stdio, exit};
use std::time::Duration;

use fluzo_core::policy::{
    Denial, DenialScope, ExecutionBackend, Grant, GrantLifetime, GrantState, PolicyOutcome,
    PolicyReason, PolicySubject, ResolvedPermission, ShellScope, SubjectKind,
};
use fluzo_core::settings::{Permission, Settings};

use crate::session_auth::{
    ApprovalMode, AuthOutcome, AuthorizationService, HeadlessApprovalRequired,
};

const WS: &str = "3f2a9c14be07d5a8";

fn ask() -> ResolvedPermission {
    ResolvedPermission {
        value: Permission::Ask,
        origin: fluzo_core::policy::PermissionOrigin::BuiltIn,
        ignored_repository_broaden: None,
    }
}

fn shell_subject() -> PolicySubject {
    PolicySubject::Shell(ShellScope::direct("git", vec!["status".to_owned()], "."))
}

fn issued_grant(subject: PolicySubject) -> Grant {
    Grant {
        id: "g1".to_owned(),
        subject,
        workspace: WS.to_owned(),
        backend: ExecutionBackend::NativeLocal,
        lifetime: GrantLifetime::OneTime,
        state: GrantState::Issued,
    }
}

// --- mode selection ------------------------------------------------------

#[test]
fn an_interactive_host_is_asked() {
    let service = AuthorizationService::new(ApprovalMode::Interactive);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(matches!(outcome, AuthOutcome::ApprovalRequired(_)));
    assert_eq!(outcome.request().unwrap().id, "op-1");
}

#[test]
fn a_headless_host_returns_structurally_instead_of_asking() {
    let service = AuthorizationService::new(ApprovalMode::Headless);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(matches!(outcome, AuthOutcome::ApprovalUnavailable(_)));
    let required = HeadlessApprovalRequired::from_outcome(&outcome).expect("structured outcome");
    assert_eq!(required.code, "approval_required");
    assert!(required.summary.contains("git"));
}

#[test]
fn headless_unavailable_is_not_reported_as_a_denial() {
    let service = AuthorizationService::new(ApprovalMode::Headless);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(
        !matches!(outcome, AuthOutcome::Denied { .. }),
        "unexecutable here is not the same message as refused"
    );
}

#[test]
fn the_interactive_path_does_not_produce_a_headless_outcome() {
    let service = AuthorizationService::new(ApprovalMode::Interactive);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(HeadlessApprovalRequired::from_outcome(&outcome).is_none());
}

// --- yolo ---------------------------------------------------------------

#[test]
fn yolo_is_off_until_turned_on_and_off_again_by_default() {
    let service = AuthorizationService::new(ApprovalMode::Interactive);
    assert!(!service.yolo_enabled(), "a new session is never YOLO");
    let mut service = AuthorizationService::new(ApprovalMode::Interactive);
    service.set_yolo(true);
    assert!(service.yolo_enabled());
    service.set_yolo(false);
    assert!(!service.yolo_enabled());
}

#[test]
fn yolo_lets_an_otherwise_asked_operation_through() {
    let mut service = AuthorizationService::new(ApprovalMode::Headless);
    service.set_yolo(true);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(outcome.allows());
}

#[test]
fn yolo_is_attributed_to_the_mode_not_to_a_user_approval() {
    let mut service = AuthorizationService::new(ApprovalMode::Headless);
    service.set_yolo(true);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert_eq!(
        outcome_reason(&outcome),
        PolicyReason::YoloSuppressesApproval,
        "the audit trail must not read as if a human approved this"
    );
    assert_eq!(
        service.ledger().live_grant_count(),
        0,
        "YOLO issues no grant"
    );
}

#[test]
fn yolo_never_overturns_a_policy_deny() {
    let mut service = AuthorizationService::new(ApprovalMode::Headless);
    service.set_yolo(true);
    let permission = ResolvedPermission {
        value: Permission::Deny,
        origin: fluzo_core::policy::PermissionOrigin::Repository,
        ignored_repository_broaden: None,
    };
    let outcome = service.resolve(
        "op-1",
        &PolicySubject::Git(fluzo_core::policy::GitOperation::Push),
        WS,
        ExecutionBackend::NativeLocal,
        &permission,
    );
    assert!(matches!(outcome, AuthOutcome::Denied { .. }));
}

#[test]
fn yolo_never_overturns_an_explicit_user_denial() {
    let mut service = AuthorizationService::new(ApprovalMode::Headless);
    service.set_yolo(true);
    service
        .ledger_mut()
        .add_denial(Denial {
            id: "d1".to_owned(),
            scope: DenialScope::Kind(SubjectKind::Shell),
        })
        .expect("denial");
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert_eq!(
        outcome_reason(&outcome),
        PolicyReason::DeniedByUser,
        "a denial the operator gave outlasts YOLO"
    );
}

#[test]
fn yolo_does_not_consume_a_grant_that_was_never_needed() {
    let mut service = AuthorizationService::new(ApprovalMode::Headless);
    service
        .ledger_mut()
        .issue(issued_grant(shell_subject()))
        .expect("issue");
    service.set_yolo(true);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    // The grant matched first, so YOLO never applied and the grant is
    // still live for its own one-time use.
    assert!(matches!(
        outcome_reason(&outcome),
        PolicyReason::GrantMatches { .. }
    ));
    assert_eq!(service.ledger().live_grant_count(), 1);
}

#[test]
fn without_yolo_the_same_subject_still_needs_approval() {
    let service = AuthorizationService::new(ApprovalMode::Headless);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(matches!(outcome, AuthOutcome::ApprovalUnavailable(_)));
}

// --- no prompt for denied work -------------------------------------------

#[test]
fn a_denied_operation_produces_no_prompt_in_an_interactive_host() {
    let mut service = AuthorizationService::new(ApprovalMode::Interactive);
    service
        .ledger_mut()
        .add_denial(Denial {
            id: "d1".to_owned(),
            scope: DenialScope::Kind(SubjectKind::Shell),
        })
        .expect("denial");
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(matches!(outcome, AuthOutcome::Denied { .. }));
    assert!(outcome.request().is_none(), "no request may be rendered");
    assert!(!service.can_approve(&shell_subject()));
}

#[test]
fn a_granted_operation_produces_no_prompt() {
    let mut service = AuthorizationService::new(ApprovalMode::Interactive);
    service
        .ledger_mut()
        .issue(issued_grant(shell_subject()))
        .expect("issue");
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    assert!(outcome.allows());
    assert!(outcome.request().is_none());
}

// --- persistence ---------------------------------------------------------

#[test]
fn there_is_no_yolo_setting_a_repository_can_write() {
    // `.fluzo` must not be able to turn YOLO on. The settings schema
    // rejects the key outright.
    let toml = r#"
schema_version = 1
yolo = true
"#;
    let parsed: Result<Settings, _> = toml_edit::de::from_str(toml);
    let error = parsed.expect_err("yolo is not a settings key");
    let text = error.to_string();
    assert!(
        text.contains("yolo"),
        "the error should name the offending key: {text}"
    );
}

#[test]
fn there_is_no_nested_yolo_setting_either() {
    let toml = r#"
schema_version = 1

[policy]
yolo = true
"#;
    let parsed: Result<Settings, _> = toml_edit::de::from_str(toml);
    assert!(parsed.is_err(), "policy.yolo is not a key either");
}

// --- non-blocking headless, as a separate process -------------------------

/// A headless resolve must return without ever touching stdin.
///
/// The child gets a pipe on stdin that the parent keeps open and never
/// writes to, so a blocking read would wait forever rather than see
/// EOF. The child must decide and exit without reading it. The parent
/// waits on a channel with a deadline, so a hang fails this test with a
/// message instead of hanging the suite.
#[test]
fn headless_resolve_returns_with_stdin_open_and_silent() {
    let (reader, _writer) = std::io::pipe().expect("pipe");
    let mut child = Command::new(std::env::current_exe().expect("test exe"))
        .arg("--ignored")
        .arg("--exact")
        .arg("session_auth_tests::headless_child")
        .arg("--nocapture")
        .env("FLUZO_TEST_ROLE", "headless_resolve")
        .stdin(Stdio::from(reader))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child");

    let stdout = child.stdout.take().expect("child stdout");
    let stderr = child.stderr.take().expect("child stderr");
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut buffer = String::new();
        let mut reader = stdout;
        let _ = reader.read_to_string(&mut buffer);
        let mut err = String::new();
        let mut err_reader = stderr;
        let _ = err_reader.read_to_string(&mut err);
        let _ = sender.send((buffer, err));
    });

    match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok((out, err)) => {
            assert!(
                out.contains("DECIDED"),
                "the child never reported a decision. stdout={out:?} stderr={err:?}"
            );
            assert!(
                !err.contains("stdin"),
                "unexpected stdin complaint: {err:?}"
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            child.kill().ok();
            panic!(
                "headless resolve blocked: the child produced nothing while stdin \
                 stayed open and silent"
            );
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("child reader thread died");
        }
    }

    let status = child.wait().expect("wait for child");
    assert!(status.success(), "child exited with {status}");
}

/// The child half of the non-blocking test. Ignored so it never runs
/// standalone; driven with `--ignored --exact` by the parent.
///
/// It never reads stdin. If the implementation ever needs stdin to
/// decide, it will block before printing and the parent's deadline
/// fails the test.
#[test]
#[ignore = "child process driven by headless_resolve_returns_with_stdin_open_and_silent"]
fn headless_child() {
    assert_eq!(
        std::env::var("FLUZO_TEST_ROLE").as_deref(),
        Ok("headless_resolve"),
        "this test is only meaningful as the driven child"
    );
    let service = AuthorizationService::new(ApprovalMode::Headless);
    let outcome = service.resolve(
        "op-1",
        &shell_subject(),
        WS,
        ExecutionBackend::NativeLocal,
        &ask(),
    );
    if !matches!(outcome, AuthOutcome::ApprovalUnavailable(_)) {
        eprintln!("unexpected outcome: {outcome:?}");
        exit(1);
    }
    println!("DECIDED");
    exit(0);
}

fn outcome_reason(outcome: &AuthOutcome) -> PolicyReason {
    match outcome {
        AuthOutcome::Allowed { reason } | AuthOutcome::Denied { reason } => reason.clone(),
        AuthOutcome::ApprovalRequired(request) | AuthOutcome::ApprovalUnavailable(request) => {
            request.reason.clone()
        }
    }
}

/// The decision the service applies YOLO to must be the ledger's own.
#[test]
fn resolve_uses_the_ledger_decision_unchanged_when_yolo_is_off() {
    let mut service = AuthorizationService::new(ApprovalMode::Interactive);
    service
        .ledger_mut()
        .issue(issued_grant(shell_subject()))
        .expect("issue");
    let decision =
        service
            .ledger()
            .decide(&shell_subject(), WS, ExecutionBackend::NativeLocal, &ask());
    let outcome = service.resolve_decision("op-1", &shell_subject(), decision.clone());
    assert_eq!(outcome_reason(&outcome), decision.reason);
    assert_eq!(decision.outcome, PolicyOutcome::Allow);
}
