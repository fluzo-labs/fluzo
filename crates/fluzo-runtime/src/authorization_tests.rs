use fluzo_core::policy::{
    Denial, DenialScope, ExecutionBackend, FileOperation, FileScope, FileVersion, Grant,
    GrantLifetime, GrantState, PolicySubject, ShellMode, ShellScope,
};
use fluzo_core::settings::Permission;

use crate::authorization::{AuthorizationLedger, LedgerError};

const WS: &str = "3f2a9c14be07d5a8";

fn ask() -> fluzo_core::policy::ResolvedPermission {
    fluzo_core::policy::ResolvedPermission {
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

fn git_status() -> PolicySubject {
    PolicySubject::Shell(ShellScope::direct("git", vec!["status".to_owned()], "."))
}

// --- exact scope: shell --------------------------------------------------

#[test]
fn a_shell_grant_does_not_cover_one_extra_argument() {
    let ledger = ledger_with(grant("g1", git_status()));
    let with_extra = PolicySubject::Shell(ShellScope::direct(
        "git",
        vec!["status".to_owned(), "--porcelain".to_owned()],
        ".",
    ));
    assert!(
        ledger
            .grant_for(&with_extra, WS, ExecutionBackend::NativeLocal)
            .is_none(),
        "approval must not extend to extra arguments"
    );
}

#[test]
fn a_shell_grant_does_not_cover_a_command_prefix() {
    let ledger = ledger_with(grant(
        "g1",
        PolicySubject::Shell(ShellScope::direct(
            "git",
            vec!["commit".to_owned(), "-m".to_owned(), "msg".to_owned()],
            ".",
        )),
    ));
    // The same executable with a prefix of the approved arguments is a
    // different command, not a subset of one.
    let prefix = PolicySubject::Shell(ShellScope::direct("git", vec!["commit".to_owned()], "."));
    assert!(
        ledger
            .grant_for(&prefix, WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
}

#[test]
fn a_shell_grant_does_not_cover_a_different_working_directory() {
    let ledger = ledger_with(grant("g1", git_status()));
    let elsewhere =
        PolicySubject::Shell(ShellScope::direct("git", vec!["status".to_owned()], "sub"));
    assert!(
        ledger
            .grant_for(&elsewhere, WS, ExecutionBackend::NativeLocal)
            .is_none(),
        "approval must not extend to other directories"
    );
}

#[test]
fn a_direct_grant_does_not_cover_the_same_command_through_a_shell() {
    let ledger = ledger_with(grant("g1", git_status()));
    let mut shelled = ShellScope::direct("git", vec!["status".to_owned()], ".");
    shelled.mode = ShellMode::ExplicitShell;
    assert!(
        ledger
            .grant_for(
                &PolicySubject::Shell(shelled),
                WS,
                ExecutionBackend::NativeLocal
            )
            .is_none(),
        "a shell interprets its argument; approving the direct form says nothing about it"
    );
}

#[test]
fn a_shell_grant_does_not_cover_different_environment_overrides() {
    let ledger = ledger_with(grant("g1", git_status()));
    let mut with_env = ShellScope::direct("git", vec!["status".to_owned()], ".");
    with_env
        .environment
        .insert("GIT_AUTHOR_EMAIL".to_owned(), "a@b.c".to_owned());
    assert!(
        ledger
            .grant_for(
                &PolicySubject::Shell(with_env),
                WS,
                ExecutionBackend::NativeLocal
            )
            .is_none()
    );
}

#[test]
fn the_exact_command_is_covered() {
    let ledger = ledger_with(grant("g1", git_status()));
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_some()
    );
}

// --- exact scope: filesystem --------------------------------------------

#[test]
fn a_filesystem_grant_does_not_cover_a_sibling_parent_or_child() {
    let ledger = ledger_with(grant(
        "g1",
        PolicySubject::Filesystem(FileScope::new(FileOperation::Write, "src/lib.rs")),
    ));
    for path in ["src/other.rs", "src.rs", "src/nested/lib.rs", "lib.rs"] {
        let subject = PolicySubject::Filesystem(FileScope::new(FileOperation::Write, path));
        assert!(
            ledger
                .grant_for(&subject, WS, ExecutionBackend::NativeLocal)
                .is_none(),
            "{path} is not the approved path"
        );
    }
}

#[test]
fn a_write_grant_does_not_cover_a_read_of_the_same_path() {
    let ledger = ledger_with(grant(
        "g1",
        PolicySubject::Filesystem(FileScope::new(FileOperation::Write, "a.txt")),
    ));
    let read = PolicySubject::Filesystem(FileScope::new(FileOperation::Read, "a.txt"));
    assert!(
        ledger
            .grant_for(&read, WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
}

#[test]
fn a_version_bound_grant_stops_covering_when_the_file_moved() {
    let version = FileVersion {
        device: 1,
        inode: 42,
        size: 100,
        modified_ns: 7,
    };
    let subject = PolicySubject::Filesystem(
        FileScope::new(FileOperation::Write, "a.txt").with_observed(version),
    );
    let ledger = ledger_with(grant("g1", subject.clone()));
    assert!(
        ledger
            .grant_for(&subject, WS, ExecutionBackend::NativeLocal)
            .is_some()
    );

    let changed = PolicySubject::Filesystem(
        FileScope::new(FileOperation::Write, "a.txt").with_observed(FileVersion {
            size: 101,
            ..version
        }),
    );
    assert!(
        ledger
            .grant_for(&changed, WS, ExecutionBackend::NativeLocal)
            .is_none(),
        "an external edit invalidates the approval"
    );
}

#[test]
fn a_version_bound_grant_does_not_cover_an_unversioned_proposal() {
    let version = FileVersion {
        device: 1,
        inode: 42,
        size: 100,
        modified_ns: 7,
    };
    let bound = PolicySubject::Filesystem(
        FileScope::new(FileOperation::Write, "a.txt").with_observed(version),
    );
    let unbound = PolicySubject::Filesystem(FileScope::new(FileOperation::Write, "a.txt"));
    let ledger = ledger_with(grant("g1", bound));
    assert!(
        ledger
            .grant_for(&unbound, WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
}

// --- one-time consumption ------------------------------------------------

#[test]
fn a_one_time_grant_covers_exactly_one_dispatch() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_some()
    );
    ledger.consume("g1", "d1").expect("consume");
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_none(),
        "the second dispatch finds nothing"
    );
}

#[test]
fn consuming_twice_is_refused_naming_the_first_dispatch() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    ledger.consume("g1", "d1").expect("first consume");
    let error = ledger
        .consume("g1", "d2")
        .expect_err("a consumed grant cannot be consumed again");
    assert_eq!(
        error,
        LedgerError::AlreadyConsumed {
            dispatch_id: "d1".to_owned()
        }
    );
    // The first dispatch record is untouched by the failed attempt.
    assert_eq!(
        ledger.grant("g1").map(|g| g.state.clone()),
        Some(GrantState::Consumed {
            dispatch_id: "d1".to_owned()
        })
    );
}

#[test]
fn consuming_an_unknown_grant_is_refused() {
    let mut ledger = AuthorizationLedger::new();
    assert_eq!(
        ledger.consume("nope", "d1").unwrap_err(),
        LedgerError::UnknownGrant
    );
}

#[test]
fn uncertainty_keeps_the_grant_spent() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    ledger.consume("g1", "d1").expect("consume");
    ledger.mark_outcome_uncertain("g1").expect("mark");
    assert!(ledger.is_uncertain("g1"));
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_none(),
        "uncertainty never re-arms the grant"
    );
}

#[test]
fn marking_uncertainty_on_an_unconsumed_grant_is_refused() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    assert!(ledger.mark_outcome_uncertain("g1").is_err());
}

// --- expiry and revocation ----------------------------------------------

#[test]
fn task_grants_expire_on_task_end_and_are_not_restored() {
    let mut ledger = AuthorizationLedger::new();
    ledger
        .issue(Grant {
            id: "t1".to_owned(),
            subject: git_status(),
            workspace: WS.to_owned(),
            backend: ExecutionBackend::NativeLocal,
            lifetime: GrantLifetime::Task {
                task_id: "task-1".to_owned(),
            },
            state: GrantState::Issued,
        })
        .expect("issue");
    ledger
        .issue(Grant {
            id: "t2".to_owned(),
            subject: PolicySubject::Git(fluzo_core::policy::GitOperation::Commit),
            workspace: WS.to_owned(),
            backend: ExecutionBackend::NativeLocal,
            lifetime: GrantLifetime::Task {
                task_id: "task-2".to_owned(),
            },
            state: GrantState::Issued,
        })
        .expect("issue");

    assert_eq!(ledger.expire_task("task-1"), 1);
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
    assert_eq!(ledger.live_grant_count(), 1, "the other task is untouched");

    // A fresh ledger after recovery has nothing to restore.
    let recovered = AuthorizationLedger::new();
    assert!(
        recovered
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
}

#[test]
fn revocation_stops_future_dispatches() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    let revoked = ledger
        .revoke("g1", "operator changed mind")
        .expect("revoke");
    assert!(matches!(revoked.state, GrantState::Revoked { .. }));
    assert!(
        ledger
            .grant_for(&git_status(), WS, ExecutionBackend::NativeLocal)
            .is_none()
    );
}

#[test]
fn revoking_a_consumed_grant_does_not_undo_the_dispatch() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    ledger.consume("g1", "d1").expect("consume");
    let error = ledger
        .revoke("g1", "too late")
        .expect_err("a consumed grant is not revocable");
    assert!(matches!(error, LedgerError::NotIssued { .. }));
    assert_eq!(
        ledger.grant("g1").map(|g| g.state.clone()),
        Some(GrantState::Consumed {
            dispatch_id: "d1".to_owned()
        })
    );
}

// --- composed decision ---------------------------------------------------

#[test]
fn decide_puts_the_denial_before_the_grant() {
    let mut ledger = ledger_with(grant("g1", git_status()));
    ledger
        .add_denial(Denial {
            id: "x1".to_owned(),
            scope: DenialScope::Exact(Box::new(git_status())),
        })
        .expect("add denial");
    let decision = ledger.decide(&git_status(), WS, ExecutionBackend::NativeLocal, &ask());
    assert_eq!(decision.outcome, fluzo_core::policy::PolicyOutcome::Deny);
    assert_eq!(
        decision.reason,
        fluzo_core::policy::PolicyReason::DeniedByUser
    );
}

#[test]
fn decide_falls_through_to_the_permission_when_nothing_matches() {
    let ledger = AuthorizationLedger::new();
    let decision = ledger.decide(&git_status(), WS, ExecutionBackend::NativeLocal, &ask());
    assert_eq!(decision.outcome, fluzo_core::policy::PolicyOutcome::AskUser);
}

#[test]
fn a_denied_subject_reports_denied_so_the_headless_path_can_return_at_once() {
    let mut ledger = AuthorizationLedger::new();
    ledger
        .add_denial(Denial {
            id: "x1".to_owned(),
            scope: DenialScope::Kind(fluzo_core::policy::SubjectKind::Shell),
        })
        .expect("add denial");
    assert!(ledger.subject_is_denied(&git_status()));
    assert!(!ledger.subject_is_denied(&PolicySubject::Git(
        fluzo_core::policy::GitOperation::Commit
    )));
}

#[test]
fn duplicate_ids_are_refused() {
    let mut ledger = AuthorizationLedger::new();
    ledger.issue(grant("g1", git_status())).expect("issue");
    assert_eq!(
        ledger.issue(grant("g1", git_status())).unwrap_err(),
        LedgerError::DuplicateId
    );
}

fn ledger_with(grant: Grant) -> AuthorizationLedger {
    let mut ledger = AuthorizationLedger::new();
    ledger.issue(grant).expect("issue");
    ledger
}
