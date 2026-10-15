---
name: "2026-10-15-ui04-task-permission-diff-demo-states"
description: "UI-04 adds isolated demo states for tasks, permissions, diffs and task notifications to the existing synthetic shell: a read-only task list and detail, distinct graceful cancel versus confirmed force stop, a permission prompt that never preselects an affirmative, a diff and verification review that cannot stage or revert, and task-severity notices, all visibly synthetic and sanitized."
created_at: "2026-10-15T05:55:20Z"
has_completed_all_phases: false
---

# UI-04: task, permission, diff and notification demo states

## Source and revision

- Plan identifier: `ui04-task-permission-diff-demo-states`.
- Source: [fluzo-labs/fluzo#9](https://github.com/fluzo-labs/fluzo/issues/9)
  (seed UI-04, M2 Visual Prototype, P1, area tui). The issue acceptance checklist
  is the contract.
- Reviewed revision: issue `updatedAt` `2026-09-24T12:53:23Z`; branch
  `feat/ui04-demo-states` from `main` at
  `fdc27800b4ae8c4e40f8846012cea2593ffa7e9d`.
- Design baseline: PRD 12 (policy engine, 12.1 normal mode, 12.2 YOLO),
  PRD 17 (task system, 17.1 inspection, 17.2 graceful cancel and force stop),
  PRD 29 (TUI, 29.2.3 untrusted terminal content, 29.3 notifications,
  29.4 acceptance), PRD 30 (TUI views: Conversation, Permissions, Diff and
  verification, Tasks) and PRD 31 (keyboard navigation), plus architecture A05
  and A07 UI parts, all at `fluzo-docs` revision
  `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: pending. This plan is a design snapshot; nothing here is accepted.

## Goal

A reviewer can open a Tasks view in the existing interactive demo, select a
synthetic task and read its identity, state, reason, current action, elapsed
budget, pending interventions, partial diff, verification and trace reference
without anything being resumed; can reach graceful cancel and force stop as two
separate controls with different confirmation requirements; can open a permission
prompt whose default is never an approval; and sees every one of these results
labelled as synthetic. All of it sanitized and reachable by keyboard at 80x24,
120x40 and 160x50.

## Scope and exclusions

In scope: new pure view-state and reducers in `fluzo-tui`, thin overlay wiring in
the existing `Shell`, deterministic buffer/unit tests, focused PTY cases, and
`TUI.md` documentation.

Excluded, because they belong to RUN/AGT/REL work: real task transport, command
dispatch, permission enforcement, provider calls, process-group termination,
session persistence, real diff reading from Git, YOLO activation, and runtime
integration of any kind. The demo shell holds no `ApplicationPort`, so it cannot
dispatch; this plan preserves that property rather than weakening it.

## Verified context

Observed in the tree at the baseline, not assumed:

- `crates/fluzo-tui/src/shell.rs` (5107 lines) owns `Shell`, the `Overlay`
  enum (`Palette`, `Help`, `Quit`, `Theme`, `Developer`, `Models`, `Sessions`,
  `Details`), the incremental `Sanitizer` (line 63) that handles split escape
  sequences, bidirectional controls and OSC, and `Shell::key` (line 1248).
- `Shell::new` (line 293) requires a valid `demo: true` snapshot with at most 64
  validated tasks. `Shell` has **no** `ApplicationPort` field anywhere: the TUI
  cannot submit commands. `crates/fluzo-tui/src/inspection.rs` is the only place
  that reads a port, and its fixture panics on `command`, `subscribe`, `poll` and
  `detach` to pin that selection is read-only.
- Top-level `Ctrl+T` / `Ctrl+Space` currently returns the dead-end status
  "Task panel is unavailable; no runtime connected." (shell.rs ~line 1560). That
  is the natural entry point for the Tasks view.
- `fluzo_core::application` already carries everything the views need:
  `TaskState` (Pending..Cancelled), `WaitReason` (WorkspaceOwnership,
  ModelCapacity, Permission, UserInput, Budget, Configuration, UncertainOutcome,
  TerminationUnconfirmed), `VerificationState` (NotRun/Passed/Failed/Waived),
  `AttemptState`, `OperationState`, `TaskSnapshot` with `session`, `workspace`,
  `version`, `active_milliseconds` and `title`, and `Command::{Cancel,
  ForceStop{confirmed}, Approve}`.
- `notification_stack::Severity` already includes `Approval`, `Warning`, `Error`
  and `Information`, with bounded stacking, priority-preserving overflow and
  focus-safe rendering from C2.
- `crates/fluzo-cli/src/main.rs::demo_scenes` builds seven host-driven scenes
  over one task by mutating state/reason/verification. `ScenarioDriver::evidence()`
  reports `accepted` request counts, which lets a test prove zero commands were
  submitted.
- `presentation.rs` has `wrap`, `inline`, `markdown`, `compact` and layout
  helpers; there is no diff renderer yet.
- Existing overlay rendering (`render_overlay`, line 1962) builds a title plus a
  `Vec<Line>` body and a footer per overlay, reusing `render_dialog_frame` and
  `render_dialog_title`. New overlays follow that shape rather than adding a
  rendering framework.

Hypothesis to confirm during implementation, not assumed: none material. The
contract is fully expressible with existing core types.

## Decisions, dependencies, and risks

- **Decision: demo actions are recorded, never dispatched.** Cancel and force stop
  update view state and produce a visibly synthetic outcome line. They cannot
  reach the runtime because the shell holds no port. Tests assert the scenario
  driver's accepted-request count stays zero after the whole demo interaction.
- **Decision: safe default is structural, not advisory.** Confirmation and
  permission option lists put the non-destructive choice at index 0 and initialise
  selection to 0, so an accidental Enter denies rather than approves. PRD 31
  requires that opening an approval dialog must not preselect an affirmative
  action; encoding it as index 0 makes it testable.
- **Decision: reuse `Ctrl+T` for Tasks.** It is currently a dead end in the
  top-level handler and is free outside the Developer overlay, where `Ctrl+T`
  keeps its notification-test meaning. Documented in `TUI.md`.
- Dependency: UI-01 (#6) merged, which supplies the shell, composer and palette.
  C1/C2/C3 visual controls, notification isolation and the shared save/apply
  route are already merged on `main`.
- Risk: shell.rs is large. Mitigation: keep all logic in a new `tasks.rs` module
  with its own unit tests; shell changes are limited to the `Overlay` variants,
  delegation and thin rendering.
- Risk: confusing a demo control with a real one. Mitigation: every projected
  string carries an explicit synthetic marker and the footer states that no
  runtime is connected.
- Risk: untrusted content in task titles or diff lines. Mitigation: all
  projections pass through the existing `Sanitizer` / `setup::safe_text`, with
  OSC 52, OSC 8, split-sequence and bidirectional cases in the fixtures.

## Phases

### P1: Read-only task, permission and diff demo states in the existing shell

- Outcome: `Ctrl+T` opens a Tasks view over the synthetic snapshot. Selecting a
  task and pressing Enter opens its detail without resuming. From the detail a
  reviewer can reach graceful cancel, force stop with a confirmation whose
  default is "Keep running", a permission prompt whose default is "Deny", and a
  diff and verification review that states reviewing does not stage, revert,
  commit or accept. Every line is labelled synthetic and sanitized.
- Owning repository: `fluzo-labs/fluzo`, crate `fluzo-tui`, plus `TUI.md`.
- Dependencies: UI-01 (#6) merged; C2 notification stack merged.
- Affected contracts: new `fluzo_tui::tasks` module (`TaskRow`, `project`,
  `state_label`, `reason_label`, `verification_label`, `TaskAction`,
  `Confirmation`, `PermissionPrompt`, `PermissionDecision`, `DiffView`,
  `ChangeOrigin`, `TaskDemo`); new `Shell::Overlay` variants `Tasks`,
  `TaskConfirm`, `Permission`, `Diff`; no change to `fluzo-core` protocol types.
- Acceptance criteria:
  1. Selecting a task does not resume it: selection mutates only the selected
     index; opening detail issues nothing; the scenario driver records zero
     accepted commands across the full interaction; the detail view states that
     no writer ownership was taken.
  2. Cancel and force stop are distinct: two separate actions with different
     confirmation requirements; graceful cancel needs a deliberate row
     selection, force stop additionally needs a confirmation whose initial
     selection is the non-destructive option, and only an explicit move to the
     destructive row yields a `confirmed: true` force stop. Repeating
     cancellation is idempotent and never escalates.
  3. Permission focus is safe: the prompt opens with the deny option selected;
     a new synthetic message arriving while the prompt is open changes neither
     focus nor selection; Enter while the composer holds a draft does not reach
     the prompt; untrusted text cannot create an actionable approval row.
  4. Content is sanitized: task titles, diff lines and permission scope pass
     through the incremental sanitizer; OSC 52, OSC 8, cursor/erase, split
     sequences and bidirectional path spoofing produce no terminal-control
     bytes in the rendered buffer.
  5. Every simulated result is visibly synthetic: list rows, detail,
     confirmation, permission decision outcome and diff review each carry an
     explicit synthetic marker and a "no runtime connected" statement.
  6. Keyboard-only reachability at 80x24, 120x40 and 160x50, plus the existing
     minimum-size behaviour; safe paste; Esc closes an overlay without
     cancelling anything.
- Actions:
  - [ ] Add `crates/fluzo-tui/src/tasks.rs` with the pure projections, the
        confirmation and permission state machines, the diff/verification
        review model and their unit tests, including the negative cases above.
  - [ ] Wire the four new overlays into `Shell` key handling and rendering with
        no new rendering framework and no port access.
  - [ ] Add task-severity synthetic notices through the existing
        `NotificationStack` without focus stealing.
  - [ ] Add focused PTY cases in `scripts/test_tui.py` for opening Tasks,
        inspecting without resuming, the force-stop confirmation default, the
        permission default, and terminal restoration.
  - [ ] Update `TUI.md` with the new demo states, how to reach them, and what
        they deliberately do not do.
  - [ ] Run the full verification set and record actual results.
  - [ ] Present evidence and stop for review.
- Verification: pinned toolchain 1.98.0, `--locked --offline`, from the worktree
  root: `cargo fmt --all -- --check`; `cargo test --workspace --locked
  --offline`; `cargo clippy --workspace --all-targets --locked --offline -- -D
  warnings`; `python3 -B -m unittest discover -s scripts -p 'test_*.py'`;
  `python3 scripts/check_dev_setup.py`. Live inference `not_run`.
- Risks and recovery: no migration and no persisted state, so recovery is
  reverting the commit. Overlay key handling is the main regression surface; the
  existing PTY suite covers focus, paste, resize and restoration and must stay
  green unchanged.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | pending | not_run | pending | none yet |

## Next step

Execute P1 of this plan at
`.agents/plans/2026-10-15-ui04-task-permission-diff-demo-states/2026-10-15-ui04-task-permission-diff-demo-states-plan.md`.
It is the only phase and has no unmet prerequisite: UI-01, C2 and C3 are merged on
`main`. Alternative: stop.

## Evidence and tracking

- Issue: [#9](https://github.com/fluzo-labs/fluzo/issues/9).
- Delivery PR: to be created with `Ref: #9`, never `Closes`.
- Documentation: `TUI.md` gains the demo-state section.
- GitHub remains the backlog authority; this file is a design snapshot, not a
  parallel board.
