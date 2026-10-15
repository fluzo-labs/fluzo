use crate::policy::*;
use crate::settings::Permission;

fn layer(value: Permission, origin: PermissionOrigin) -> PermissionLayer {
    PermissionLayer::new(value, origin)
}

fn ask() -> ResolvedPermission {
    ResolvedPermission {
        value: Permission::Ask,
        origin: PermissionOrigin::BuiltIn,
        ignored_repository_broaden: None,
    }
}

fn shell_subject() -> PolicySubject {
    PolicySubject::Shell(ShellScope::direct("git", vec!["status".to_owned()], "."))
}

fn grant_for(subject: PolicySubject, workspace: &str) -> Grant {
    Grant {
        id: "grant-1".to_owned(),
        subject,
        workspace: workspace.to_owned(),
        backend: ExecutionBackend::NativeLocal,
        lifetime: GrantLifetime::OneTime,
        state: GrantState::Issued,
    }
}

// --- repository authority -------------------------------------------------

#[test]
fn repository_allow_cannot_widen_an_operator_ask() {
    let resolved = resolve_permission(&[
        layer(Permission::Ask, PermissionOrigin::BuiltIn),
        layer(Permission::Allow, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Ask, "a repo cannot authorize");
    assert_eq!(
        resolved.ignored_repository_broaden,
        Some(Permission::Allow),
        "the attempt is recorded, not silently dropped"
    );
    assert_eq!(resolved.origin, PermissionOrigin::BuiltIn);
}

#[test]
fn repository_allow_cannot_widen_an_operator_deny() {
    let resolved = resolve_permission(&[
        layer(Permission::Deny, PermissionOrigin::ExplicitFile),
        layer(Permission::Allow, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Deny);
    assert_eq!(resolved.ignored_repository_broaden, Some(Permission::Allow));
}

#[test]
fn repository_deny_narrows_an_operator_allow() {
    let resolved = resolve_permission(&[
        layer(Permission::Allow, PermissionOrigin::CommandLine),
        layer(Permission::Deny, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Deny);
    assert_eq!(
        resolved.origin,
        PermissionOrigin::Repository,
        "the operator can see who narrowed it"
    );
    assert_eq!(resolved.ignored_repository_broaden, None);
}

#[test]
fn repository_ask_narrows_an_operator_allow() {
    let resolved = resolve_permission(&[
        layer(Permission::Allow, PermissionOrigin::ExplicitFile),
        layer(Permission::Ask, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Ask);
    assert_eq!(resolved.origin, PermissionOrigin::Repository);
}

#[test]
fn repository_layer_equal_to_the_operator_value_is_not_a_broaden() {
    let resolved = resolve_permission(&[
        layer(Permission::Ask, PermissionOrigin::ExplicitFile),
        layer(Permission::Ask, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Ask);
    assert_eq!(resolved.ignored_repository_broaden, None);
    assert_eq!(resolved.origin, PermissionOrigin::ExplicitFile);
}

#[test]
fn command_line_outranks_an_explicit_file() {
    let resolved = resolve_permission(&[
        layer(Permission::Deny, PermissionOrigin::ExplicitFile),
        layer(Permission::Allow, PermissionOrigin::CommandLine),
    ]);
    assert_eq!(resolved.value, Permission::Allow);
    assert_eq!(resolved.origin, PermissionOrigin::CommandLine);
}

#[test]
fn with_no_layers_at_all_the_result_is_ask() {
    let resolved = resolve_permission(&[]);
    assert_eq!(resolved.value, Permission::Ask);
    assert_eq!(resolved.origin, PermissionOrigin::BuiltIn);
}

#[test]
fn only_the_repository_is_barred_from_broadening() {
    assert!(!PermissionOrigin::Repository.may_broaden());
    assert!(PermissionOrigin::BuiltIn.may_broaden());
    assert!(PermissionOrigin::ExplicitFile.may_broaden());
    assert!(PermissionOrigin::CommandLine.may_broaden());
}

#[test]
fn two_conflicting_repository_narrowings_take_the_stricter_effect() {
    // Both narrow; the loop applies each only if stricter than the
    // running value, so the most restrictive one ends up governing.
    let resolved = resolve_permission(&[
        layer(Permission::Allow, PermissionOrigin::ExplicitFile),
        layer(Permission::Ask, PermissionOrigin::Repository),
        layer(Permission::Deny, PermissionOrigin::Repository),
    ]);
    assert_eq!(resolved.value, Permission::Deny);
}

// --- denial precedence ----------------------------------------------------

#[test]
fn an_explicit_denial_beats_a_matching_grant() {
    let subject = shell_subject();
    let grant = grant_for(subject.clone(), "ws-1");
    let denial = Denial {
        id: "deny-1".to_owned(),
        scope: DenialScope::Exact(Box::new(subject.clone())),
    };
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &ask(),
        Some(&denial),
        Some(&grant),
    );
    assert_eq!(decision.outcome, PolicyOutcome::Deny);
    assert_eq!(decision.reason, PolicyReason::DeniedByUser);
}

#[test]
fn a_denial_by_kind_covers_every_subject_of_that_kind() {
    let denial = Denial {
        id: "deny-2".to_owned(),
        scope: DenialScope::Kind(SubjectKind::Shell),
    };
    assert!(denial.matches(&shell_subject()));
    assert!(denial.matches(&PolicySubject::Shell(ShellScope::direct(
        "anything",
        vec![],
        "anywhere"
    ))));
    assert!(!denial.matches(&PolicySubject::Git(GitOperation::Commit)));
}

#[test]
fn a_denial_for_one_exact_subject_does_not_cover_a_different_one() {
    let denial = Denial {
        id: "deny-3".to_owned(),
        scope: DenialScope::Exact(Box::new(shell_subject())),
    };
    let different = PolicySubject::Shell(ShellScope::direct("git", vec!["push".to_owned()], "."));
    assert!(!denial.matches(&different));
}

#[test]
fn a_denial_beats_an_allowing_permission() {
    let subject = PolicySubject::Git(GitOperation::Commit);
    let permission = ResolvedPermission {
        value: Permission::Allow,
        origin: PermissionOrigin::CommandLine,
        ignored_repository_broaden: None,
    };
    let denial = Denial {
        id: "deny-4".to_owned(),
        scope: DenialScope::Kind(SubjectKind::Git),
    };
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &permission,
        Some(&denial),
        None,
    );
    assert_eq!(decision.outcome, PolicyOutcome::Deny);
}

// --- grants ---------------------------------------------------------------

#[test]
fn a_live_grant_allows_the_exact_operation() {
    let subject = shell_subject();
    let grant = grant_for(subject.clone(), "ws-1");
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &ask(),
        None,
        Some(&grant),
    );
    assert_eq!(decision.outcome, PolicyOutcome::Allow);
    assert_eq!(
        decision.reason,
        PolicyReason::GrantMatches {
            grant_id: "grant-1".to_owned()
        }
    );
}

#[test]
fn a_grant_does_not_cover_a_different_workspace() {
    let subject = shell_subject();
    let grant = grant_for(subject.clone(), "ws-other");
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &ask(),
        None,
        Some(&grant),
    );
    assert_eq!(
        decision.outcome,
        PolicyOutcome::AskUser,
        "the grant is for another workspace"
    );
}

#[test]
fn a_grant_does_not_cover_a_different_backend() {
    let subject = shell_subject();
    let grant = grant_for(subject.clone(), "ws-1");
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::ExplicitShell,
        &ask(),
        None,
        Some(&grant),
    );
    assert_eq!(decision.outcome, PolicyOutcome::AskUser);
}

#[test]
fn a_consumed_grant_covers_nothing() {
    let subject = shell_subject();
    let mut grant = grant_for(subject.clone(), "ws-1");
    grant.state = GrantState::Consumed {
        dispatch_id: "d-1".to_owned(),
    };
    assert!(!grant.covers(&subject, "ws-1", ExecutionBackend::NativeLocal));
    assert!(grant.is_consumed());
}

#[test]
fn a_revoked_or_expired_grant_covers_nothing() {
    let subject = shell_subject();
    for state in [
        GrantState::Revoked {
            reason: "user".to_owned(),
        },
        GrantState::Expired,
    ] {
        let mut grant = grant_for(subject.clone(), "ws-1");
        grant.state = state;
        assert!(
            !grant.covers(&subject, "ws-1", ExecutionBackend::NativeLocal),
            "state {:?} must not cover",
            grant.state
        );
    }
}

// --- fail-closed subjects -------------------------------------------------

#[test]
fn a_subject_with_no_governing_key_fails_closed_to_ask() {
    let subject = PolicySubject::Credential {
        reference: "provider_key".to_owned(),
    };
    assert_eq!(subject.policy_key(), None);
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &ask(),
        None,
        None,
    );
    assert_eq!(decision.outcome, PolicyOutcome::AskUser);
    assert_eq!(decision.reason, PolicyReason::NoGateFailsClosed);
}

#[test]
fn an_unknown_subject_kind_fails_closed_even_when_permissions_are_permissive() {
    let subject = PolicySubject::Unknown {
        kind: "quantum_exec".to_owned(),
    };
    let permissive = ResolvedPermission {
        value: Permission::Allow,
        origin: PermissionOrigin::CommandLine,
        ignored_repository_broaden: None,
    };
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &permissive,
        None,
        None,
    );
    assert_eq!(
        decision.outcome,
        PolicyOutcome::AskUser,
        "an unknown subject is never allowed by default"
    );
}

#[test]
fn filesystem_is_not_a_permission_knob() {
    // Filesystem access is decided by the RUN-01 jail. There is no
    // `policy.filesystem` key, and this engine must not invent one that
    // would act as a weaker second authority.
    let subject = PolicySubject::Filesystem(FileScope::new(FileOperation::Write, "src/main.rs"));
    assert_eq!(subject.policy_key(), None);
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &ask(),
        None,
        None,
    );
    assert_eq!(decision.outcome, PolicyOutcome::AskUser);
}

#[test]
fn registry_keys_match_the_settings_module_defaults() {
    assert_eq!(
        PolicySubject::Shell(ShellScope::direct("x", vec![], ".")).policy_key(),
        Some("policy.shell")
    );
    assert_eq!(
        PolicySubject::Network(NetworkScope {
            destination: "example.invalid".to_owned()
        })
        .policy_key(),
        Some("policy.tool_network")
    );
    assert_eq!(
        PolicySubject::Git(GitOperation::Commit).policy_key(),
        Some("policy.git_commit")
    );
    assert_eq!(
        PolicySubject::Git(GitOperation::Push).policy_key(),
        Some("policy.git_push")
    );
}

// --- yolo ---------------------------------------------------------------

#[test]
fn yolo_suppresses_only_ask_user() {
    let asked = PolicyDecision {
        outcome: PolicyOutcome::AskUser,
        reason: PolicyReason::PolicyRequestsApproval {
            key: "policy.shell".to_owned(),
            origin: PermissionOrigin::BuiltIn,
        },
    };
    let after = apply_yolo(asked, true);
    assert_eq!(after.outcome, PolicyOutcome::Allow);
    assert_eq!(after.reason, PolicyReason::YoloSuppressesApproval);
}

#[test]
fn yolo_never_overturns_a_denial() {
    let subject = PolicySubject::Git(GitOperation::Push);
    let permission = ResolvedPermission {
        value: Permission::Deny,
        origin: PermissionOrigin::Repository,
        ignored_repository_broaden: None,
    };
    let decision = evaluate(
        &subject,
        "ws-1",
        ExecutionBackend::NativeLocal,
        &permission,
        None,
        None,
    );
    let after = apply_yolo(decision, true);
    assert_eq!(
        after.outcome,
        PolicyOutcome::Deny,
        "YOLO removes prompts, not policy"
    );
}

#[test]
fn without_yolo_the_decision_is_untouched() {
    let asked = PolicyDecision {
        outcome: PolicyOutcome::AskUser,
        reason: PolicyReason::NoGateFailsClosed,
    };
    assert_eq!(apply_yolo(asked.clone(), false), asked);
}

// --- redaction ----------------------------------------------------------

#[test]
fn environment_values_never_appear_in_the_summary() {
    let mut scope = ShellScope::direct("run", vec!["--flag".to_owned()], ".");
    scope
        .environment
        .insert("API_TOKEN".to_owned(), "super-secret-value".to_owned());
    let summary = PolicySubject::Shell(scope).redacted_summary();
    assert!(summary.contains("API_TOKEN"), "the name must be visible");
    assert!(summary.contains("[redacted]"));
    assert!(
        !summary.contains("super-secret-value"),
        "the value must never appear: {summary}"
    );
}

#[test]
fn a_credential_reference_is_shown_but_nothing_else() {
    let summary = PolicySubject::Credential {
        reference: "provider_key".to_owned(),
    }
    .redacted_summary();
    assert_eq!(summary, "credential provider_key");
}

#[test]
fn the_registry_default_for_tool_network_denies() {
    // Read the real registry value rather than restating it here, so a
    // change to the default shows up as a failing test rather than a
    // stale assertion.
    let settings = crate::settings::Settings::default();
    let descriptor = settings
        .descriptors()
        .into_iter()
        .find(|descriptor| descriptor.key == "policy.tool_network")
        .expect("policy.tool_network is a registered key");
    assert_eq!(
        descriptor.default,
        crate::settings::SettingValue::Text("deny".to_owned()),
        "tool network must default to deny"
    );

    let permission = resolve_permission(&[layer(Permission::Deny, PermissionOrigin::BuiltIn)]);
    let subject = PolicySubject::Network(NetworkScope {
        destination: "api.example.invalid".to_owned(),
    });
    let decision = evaluate(
        &subject,
        "ws-1",
        crate::policy::ExecutionBackend::NativeLocal,
        &permission,
        None,
        None,
    );
    assert_eq!(decision.outcome, PolicyOutcome::Deny);
    assert_eq!(
        apply_yolo(decision, true).outcome,
        PolicyOutcome::Deny,
        "YOLO does not open the tool network"
    );
}
