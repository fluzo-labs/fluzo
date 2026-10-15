# UI-04 multi-state task demo fixture

Issue: Ref: #77 (UI-04 review fixture). Worker W1, task T23.

## Contract

Extend the Ctrl+T demo fixture so one review session shows the full task
state space, multiple permission scopes, partial diffs with differing
verification, and a UI-04 notification set. Fixture-only: no runtime,
provider, inference or real workspace access. All existing safety
invariants must hold.

## Scope decisions

- Fixture lives in `crates/fluzo-tui/src/tasks.rs` (view data + synthetic
  constructors). Wiring lives in `shell.rs` + `terminal.rs` (demo entry).
- The interactive demo installs the fixture via a new
  `Shell::install_task_demo_fixture()` called from `run_demo`. Test shells
  created through `Shell::new` are untouched (flag stays false), so the
  existing `set_snapshot` re-projection/notice tests keep passing.
- `apply_snapshot` preserves the installed fixture (skips re-projection and
  notice re-raising) so scene playback cannot collapse the list back to the
  single-task scene.
- Permission scope is carried on `TaskRow.permission_scope` (enum
  `Credential` / `WorkspaceWrite`); `project()` defaults to `Credential`
  (current `synthetic` behaviour), the fixture sets `WorkspaceWrite` for
  the workspace-write task.
- `open_diff` marks a diff partial for `Cancelled` and `Failed` rows, so
  the failed (verification Failed) and cancelled (verification NotRun)
  tasks give two partial diffs with differing verification.

## Fixture (8 tasks)

| id | state | reason | verification | scope | diff |
|----|-------|--------|--------------|-------|------|
| 101 | Pending (queued) | - | NotRun | - | - |
| 102 | Running | - | NotRun | - | - |
| 103 | Blocked (awaiting permission) | Permission | NotRun | Credential ($FLUZO_DEMO_TOKEN) | - |
| 104 | Blocked (awaiting permission) | Permission | NotRun | WorkspaceWrite | - |
| 105 | Completed | - | Passed | - | full, passed |
| 106 | Failed | - | Failed | - | partial, failed |
| 107 | Stopping | TerminationUnconfirmed | NotRun | - | - |
| 108 | Cancelled | - | NotRun | - | partial, not run |

Notification set raised at install: Blocked->Approval (retained),
Failed->Error (retained), Completed->Success (transient),
Stopping->Warning (transient).

## Safety invariants to preserve

- Selection never resumes; Deny/Keep-running remain index 0.
- Permission scope shows references only ($FLUZO_DEMO_TOKEN), never values.
- Cancel/force-stop stay separate; force-stop needs confirmation.
- Reviewing a diff never stages, commits or pushes.
- Titles/paths sanitized; zero-width escaping preserved.
- Fixture is deterministic (fixed ids/titles/states).

## Phases

- [x] P1: tasks.rs: PermissionScope enum, workspace_write constructor,
      TaskRow.permission_scope field, project() default, open_permission by
      scope, open_diff partial for Cancelled|Failed, multi_state_fixture().
- [x] P2: shell.rs: task_demo_fixture + task_notices_raised flags,
      install_task_demo_fixture() (installs rows only), apply_snapshot
      preserves fixture, Ctrl+T open raises the UI-04 notification set once
      (lazy, so it does not pre-empt the notification-preview sequence gate).
- [x] P3: terminal.rs run_demo installs the fixture after Shell::new.
- [x] P4: regression tests (fixture shape, scopes, partial diffs, notice
      set, safety invariants on the fixture).
- [x] P5: verification.

## Verification

- `cargo fmt --all -- --check`
- `cargo test --workspace --locked --offline`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
