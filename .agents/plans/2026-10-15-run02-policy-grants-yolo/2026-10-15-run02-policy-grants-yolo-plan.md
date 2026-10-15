---
name: "2026-10-15-run02-policy-grants-yolo"
description: "Denials win, grants are exact and short-lived, headless approval returns instead of hanging, and repository config cannot authorize sensitive changes."
created_at: "2026-10-15T18:48:57Z"
has_completed_all_phases: false
---

# RUN-02: policy, grants, consent and session YOLO

## Source and revision

- Plan identifier: `run02-policy-grants-yolo`.
- Source: <https://github.com/fluzo-labs/fluzo/issues/16> (RUN-02),
  task brief `/tmp/fluzo-task-T26.md`.
- Reviewed revision: issue `updatedAt`
  `2026-09-24T12:53:31Z`; branch `feat/run02-policy-grants-yolo` at
  `610f540` (contains RUN-01 #99, the DAT chain, OBS-01 #89, endpoint
  consent #55 and FND-02 #2).
- Design baseline: PRD v0.2 sections 12 (12.1, 12.2), 26 (26.1, 26.2)
  and 42 (42.1); Architecture v0.1 decisions A05 and A06, at
  `fluzo-docs` revision `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: pending. This plan is a design draft; it does not authorize
  itself.

## Goal

A policy decision layer where an explicit denial beats any grant, a
grant only ever covers the exact operation it was given for and is
consumed before dispatch, session YOLO suppresses prompts without
suppressing policy, and a run with no terminal returns a typed
`permission_required` promptly instead of blocking on input.

## Scope and exclusions

In scope:

- Policy evaluation producing `Allow` / `Deny` / `AskUser` with typed
  reasons, in `fluzo-core` (the crate table assigns pure policy there).
- Origin-aware permission resolution: repository-origin settings are
  clamped so they can only narrow, never broaden.
- Grant identity and exact scope matching: tool/action, workspace,
  execution backend, and operation-specific scope including shell
  executable/arguments, working directory and explicit environment
  overrides.
- One-time consumption before dispatch, task-grant expiry on
  completion/failure/cancellation/interruption, revocation, and
  invalidation when the operation or a bound file version changes.
- Session YOLO: transient, explicitly activated, attributed to YOLO in
  audit rather than recorded as a user approval, and never a persisted
  startup default.
- Headless approval: bounded deadline, typed outcome, no infinite block,
  `permission_required` distinct from `permission_denied`.
- Redacted audit records for request, grant, consumption, denial, expiry
  and revocation through the existing OBS-01 audit path.

Excluded, deliberately:

- No execution loop, no agent turn machinery (the AGT chain).
- No budgets, no admission, no provider/pool capacity (#19).
- No new CLI subcommand. The CLI today is bootstrap help/version plus the
  isolated demo; there is no `fluzo run` to hang, so the headless
  contract is proven at the runtime surface with a real child process
  rather than through a command that does not exist.
- No MCP, ACP or third-party tool wiring.
- No OS sandbox. PRD 42 and A05 both say the confinement is over
  Fluzo-mediated native operations only.
- No interactive approval UI. The approval host is a port this layer
  calls; rendering it is TUI work under a later story.

## Verified context

Observed, not assumed:

- `crates/fluzo-core/src/settings.rs` already defines
  `Permission { Ask, Deny, Allow }` (default `Ask`) and a
  `PolicySettings` group with `shell`, `tool_network` (default `Deny`),
  `git_commit` and `git_push`. Descriptions already say "Requested
  shell policy, not a user grant" — the registry was written expecting
  this layer.
- `SettingOrigin { BuiltIn, Repository, ExplicitFile, CommandLine, ... }`
  already distinguishes where a value came from, which is exactly what
  the "repository cannot broaden" clamp needs.
- `crates/fluzo-runtime/src/model_discovery.rs` is the existing consent
  precedent from #55: a `Consent` struct held in process memory only,
  with the comment "nothing here is ever written to `.fluzo` or any
  other file, so every run starts unaccepted", plus a
  `consent_gate(...)` that refuses *before queueing* so no socket can be
  attempted. That is the shape to follow for YOLO's transience.
- `STORAGE.md`, "Confirmation cannot be bypassed": `Confirmation` has
  `Prompted`, `ExplicitYes`, `None`; there is deliberately no bypass
  flag, a YOLO-style caller that has not confirmed gets `None`, which
  deletes nothing and returns `confirmation_required`, and the CLI
  returns that code immediately rather than blocking on input. This is
  the precedent the task points at: YOLO does not reach this surface.
- `crates/fluzo-runtime/src/confinement.rs` (RUN-01) provides
  `WorkspaceJail` with `symlink_metadata` and confined handles, which is
  where a bound file version can be observed and re-checked.
- `crates/fluzo-runtime/src/telemetry_audit.rs` provides
  `AuditTrail::commit_with(records, updates, events)` in one
  `BEGIN IMMEDIATE` unit, and `AuditCategory::PermissionDecision`
  already exists as the fallback category.
- `crates/fluzo-core/src/application.rs` already has
  `ApplicationError::{PermissionRequired, PermissionDenied,
  ConfirmationRequired}`, so the machine-readable codes are not new.
- `crates/fluzo-cli/src/startup.rs` shows the headless pattern: a
  structured string code plus a one-line message, returned rather than
  prompting.

Not present, and treated as such:

- No task runtime, no tool dispatcher, no approval host. These are
  callers that do not exist yet, so the contract is exercised through
  the port and fixtures rather than production wiring.

## Decisions, dependencies, and risks

Decisions taken:

1. **Restriction ordering makes the clamp mechanical.**
   `Allow < Ask < Deny`. A repository-origin value is combined with the
   user-side effective value by taking the more restrictive of the two.
   This makes "the permission table is a requested per-repository
   policy, not a global authority" a one-line rule rather than a
   per-key special case, and it means a repository `allow` on a key the
   user left at `ask` cannot silently authorize anything.
2. **A clamped repository broadening is recorded, not silently dropped.**
   The decision carries `ignored_repository_broaden` so the operator can
   see that a `.fluzo` tried to widen a permission and did not get to.
   Silent dropping would make the file's behavior unknowable.
3. **Grant matching is exact equality on the scope, never prefix or
   subset.** PRD 12 says approval MUST NOT extend to command prefixes,
   extra arguments, chained commands or other directories. Implementing
   that as "the scope values must be equal" is the only version that
   holds; any matcher clever enough to generalise is the bug.
4. **Consumption is a state transition, not a boolean read.** A grant
   goes `Issued -> Consumed`. `covers()` on a consumed grant is false.
   This makes "consumed before dispatch" checkable and makes a
   double-dispatch on one grant impossible by construction.
5. **Uncertain consumption is distinct from not consumed.** If the
   process cannot tell whether a dispatch happened, the grant stays
   consumed and the operation is reported uncertain, mirroring RUN-01's
   `WriterTicket::uncertain`. A grant that could be "un-consumed" is a
   grant that can be replayed.
6. **YOLO lives in the same transience class as the #55 consent struct.**
   Process memory only. There is no settings key for it and none is
   added, so there is nothing to persist even by accident. Resume
   constructs a fresh non-YOLO state.
7. **Headless is decided from the host capability before the request,
   not by a timeout on a prompt that was never going to be answered.**
   A run with no terminal returns `permission_required` immediately.
   The timeout exists for the interactive case where a host exists but
   does not answer, and its outcome is the same typed non-approval.

Dependencies:

- RUN-01 (#15, merged as #99) for the jail used to observe and re-check
  file versions. Accepted as available, not as accepted behavior; the
  confinement tests are the evidence.
- FND-02 (#2, merged) for the typed registry, `SettingOrigin` and
  `Permission`.
- OBS-01 (#89, merged) for the audit path.

Risks:

1. **Over-building a policy language.** PRD 12 lists eight policy types.
   Implementing all eight as distinct engines would be a framework. This
   plan implements the decision rule once and gives each kind a scope
   type, which is the minimum that satisfies "grants MUST identify the
   tool/action, workspace, execution backend, and operation-specific
   scope".
2. **Persistence of grants and denials.** Architecture A05 says normal
   grants, current denials, one-time consumption and YOLO provenance
   are persisted. This plan persists the *decision and its provenance*
   as audit records and the blocked-task outcome, and deliberately does
   **not** restore grants or YOLO after a restart — which is what PRD
   12 requires ("MUST NOT be restored automatically on recovery"). If
   a reviewer wants a durable grant table beyond the audit trail, that
   is a follow-up, not a hidden gap.
3. **Proving "does not hang" without a CLI command.** Mitigated by a
   real child process that would block forever if the contract were
   broken, with the parent asserting a typed outcome and a clean child
   exit.
4. **YOLO being read from repository configuration by mistake.** There is
   no `yolo` key in the registry and this plan adds none. A `.fluzo`
   containing `yolo = true` is an unknown field and is rejected by
   `deny_unknown_fields`, which the `group!` macro already applies.

## Phases

### P1: Policy evaluation, denial precedence and repository authority limits

- Outcome: given an operation and a set of effective settings with
  origins, `evaluate` returns `Allow`, `Deny` or `AskUser` with a typed
  reason; a repository `allow` against a user-side `ask` or `deny`
  produces the user-side decision plus a recorded broaden attempt; an
  explicit denial outranks a matching grant and outranks an `allow`.
- Owning repository: fluzo-labs/fluzo, `fluzo-core`.
- Dependencies: FND-02 (#2, merged).
- Affected contracts: new `fluzo_core::policy` with `PolicySubject`,
  `PolicyDecision`, `PolicyReason`, `PermissionOrigin`,
  `effective_permission`, `evaluate`. No existing signature changes.
- Acceptance criteria:
  - A repository-origin `allow` never produces `Allow` for a key whose
    user-side effective value is `ask` or `deny`; the decision is the
    user-side one and carries `IgnoredRepositoryBroaden`.
  - A repository-origin `deny` does take effect, because narrowing is
    allowed.
  - An explicit user denial matching the operation returns `Deny` even
    when a valid grant also matches.
  - A CLI-origin value outranks a repository value in the same
    direction-only manner.
  - `tool_network` default `deny` yields `Deny` with no prompt, in YOLO
    too.
  - Unknown subject kinds fail closed to `AskUser`, never `Allow`.
- Actions:
  - [x] Implement the policy module in core with no I/O.
  - [x] Pure tests for every precedence combination.
  - [x] Record verification and stop for review.
- Implemented evidence: `fluzo-core/src/policy.rs` adds
  `PermissionOrigin`, `PermissionLayer`, `resolve_permission`,
  `ResolvedPermission` (with `ignored_repository_broaden`),
  `SubjectKind`, `PolicySubject`, `Grant`, `Denial`, `PolicyDecision`,
  `PolicyReason`, `evaluate` and `apply_yolo`. 28 tests in
  `fluzo-core/src/policy_tests.rs` cover each precedence combination,
  the recorded broaden attempt, denial-over-grant, fail-closed unknown
  and filesystem subjects, and the real registry default for
  `policy.tool_network` read from `Settings::descriptors()` rather than
  restated.
- Verification: `cargo test -p fluzo-core --lib --locked --offline
  policy`.
- Risks and recovery: additive module; removing it leaves the registry
  untouched.

### P2: Grants with exact scope, one-time consumption, expiry and revocation

- Outcome: a grant can be issued for one exact operation, matched only
  by that exact operation, consumed once before dispatch, revoked, and
  invalidated by a changed scope or a changed bound file version.
- Owning repository: fluzo-labs/fluzo, `fluzo-core` for the scope
  model and `fluzo-runtime` for the ledger.
- Dependencies: P1 for the decision type; RUN-01 (#15) for the jail
  used to re-check a bound file version.
- Affected contracts: `GrantScope`, `GrantLifetime`, `Grant` and
  `GrantId` in core; `AuthorizationLedger` in runtime with
  `issue`, `match_for`, `consume`, `revoke`, `expire_task`. No existing
  signature changes.
- Acceptance criteria:
  - A shell grant does not cover the same executable with one extra
    argument, a different working directory, a different shell mode, or
    a different environment override set.
  - A grant for one workspace-relative path does not cover a sibling, a
    parent, or a nested path.
  - A one-time grant covers exactly one dispatch; the second `match_for`
    returns nothing.
  - A task grant stops covering after `expire_task` for completion,
    failure, cancellation or interruption, and nothing is restored on a
    fresh ledger.
  - Revocation stops coverage immediately for subsequent dispatches and
    does not undo a completed dispatch.
  - A grant bound to a file version stops covering when the observed
    version differs.
  - Consumption before dispatch is observable: after `consume` the grant
    is `Consumed` and cannot be re-armed.
- Actions:
  - [x] Implement the scope model and ledger.
  - [x] Pure tests for each non-match plus the match.
  - [x] Record verification and stop for review.
- Implemented evidence: `fluzo-runtime/src/authorization.rs` adds
  `AuthorizationLedger` with `issue`, `add_denial`, `denial_for`,
  `grant_for`, `consume`, `mark_outcome_uncertain`, `revoke`,
  `expire_task` and `decide`. 23 tests in `authorization_tests.rs`
  cover extra-argument / prefix / working-directory / shell-mode /
  environment non-matches, sibling-parent-nested path non-matches,
  read-vs-write, version binding in both directions, one dispatch per
  one-time grant, `AlreadyConsumed` naming the first dispatch,
  uncertainty keeping the grant spent, task expiry isolation,
  revocation not undoing a dispatch, denial-before-grant in the composed
  decision, and duplicate-id refusal.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  grant`.
- Risks and recovery: additive; the ledger is not wired to a dispatcher.

### P3: Session YOLO and headless approval

- Outcome: YOLO suppresses `AskUser` for one session without suppressing
  `Deny`, is attributed to YOLO rather than as a user approval, cannot
  be read from or written to configuration, and is not restored by
  resume. A run with no interactive host returns `permission_required`
  promptly, and explicit denial returns `permission_denied`.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: P1 and P2.
- Affected contracts: `SessionAuthorization` (YOLO state),
  `HostCapability { Interactive, Headless }`, `ApprovalOutcome`,
  `AuthorizationService::request_approval`. No existing signature
  changes.
- Acceptance criteria:
  - With YOLO active, a subject that would be `AskUser` proceeds with
    `via = Yolo`, and the audit record says YOLO rather than user.
  - With YOLO active, a subject that is `Deny` fails visibly with
    `Denied` and produces no approval request.
  - YOLO does not bypass the workspace boundary, a stale file version, a
    consumed grant, or cancellation.
  - There is no settings key for YOLO; a `.fluzo` containing `yolo =
    true` is rejected as an unknown field.
  - Disabling YOLO re-evaluates pending operations; already-completed
    work is untouched.
  - A fresh session after resume is not YOLO, even if the previous one
    was.
  - Headless with no applicable grant returns `PermissionRequired` with
    request identity, executes nothing, and returns without waiting.
  - Headless with an explicit denial returns `PermissionDenied`, and the
    two codes are distinguishable.
  - A real child process that requests approval with no host terminates
    with a typed outcome inside a bounded window; the fixture fails if it
    is still alive.
- Actions:
  - [x] Implement YOLO state and the approval resolution surface.
  - [x] Child-process fixture proving the non-blocking contract.
  - [x] Record verification and stop for review.
- Implemented evidence: `fluzo-runtime/src/session_auth.rs` adds
  `ApprovalMode`, `ApprovalRequest`, `AuthOutcome` (with
  `ApprovalUnavailable` distinct from `Denied`), `AuthorizationService`
  and `HeadlessApprovalRequired`. 17 tests in `session_auth_tests.rs`
  cover YOLO attribution (`YoloSuppressesApproval`, zero grants
  issued), YOLO not overturning policy deny or explicit denial, YOLO
  not consuming a grant it never needed, no prompt for denied or
  granted work, `.fluzo` rejecting both `yolo` and `policy.yolo` as
  unknown fields, and the child-process non-blocking fixture.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  yolo` and `cargo test -p fluzo-runtime --test approval_headless
  --locked --offline`.
- Risks and recovery: the child fixture must not be able to hang CI; it
  carries its own hard deadline and the parent kills and fails on
  overrun.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed (28 core policy tests) | pending | `fluzo-core/src/policy.rs`, `policy_tests.rs` |
| P2 | implemented | passed (23 authorization tests) | pending | `fluzo-runtime/src/authorization.rs`, `authorization_tests.rs` |
| P3 | implemented | passed (17 session_auth tests incl. child fixture) | pending | `fluzo-runtime/src/session_auth.rs`, `session_auth_tests.rs`, `POLICY.md` |

Full workspace run after all three phases: 568 passed, 0 failed, 2
ignored (the child-process role tests, driven by their parents).
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
--locked --offline -- -D warnings`,
`python3 -B -m unittest discover -s scripts -p 'test_*.py'` (89 tests)
and `python3 scripts/check_dev_setup.py` all clean.

## Next step

Human review of the PR referencing #16 and the three children.
`implemented` is not `accepted`. Production wiring — a dispatcher that
consumes grants, and a TUI that renders `ApprovalRequired` — is later
work and is not claimed here.

## Evidence and tracking

- Parent issue: <https://github.com/fluzo-labs/fluzo/issues/16>.
- Children: created during the split step of this task; URLs recorded in
  the PR.
- GitHub issues remain the state authority; this document is a design
  snapshot, not a parallel board.
