---
name: "2026-10-15-ui05-review-package"
description: "Single-session review package for the merged UI-02/UI-03/UI-04 surfaces at the three specified viewports, with honest evidence classes and empty human slots."
created_at: "2026-10-15T11:13:56Z"
has_completed_all_phases: false
---

# First visual and interaction review package (UI-05)

## Source and revision

- Plan identifier: `2026-10-15-ui05-review-package`.
- Source: [fluzo-labs/fluzo#10](https://github.com/fluzo-labs/fluzo/issues/10)
  (UI-05, `updatedAt = 2026-09-24T12:53:24Z`) plus the task brief
  `/tmp/fluzo-task-T13.md`.
- Reviewed revision: `b0448790ee480f57f527239c6041c906034e72d0`
  (`origin/main`, contains merged UI-02 evidence #70, UI-03 matrix #69,
  UI-04 demo states #77 and DAT-02c #79).
- Reference repository and branch: `fluzo-labs/fluzo`,
  `feat/ui05-review-package` (worktree
  `/home/jose/gitrepos/fluzo-wt-ui05`).
- Design baseline: PRD v0.2 sections 29.4, 33.1 and 49, and Architecture
  v0.1 decision A07, at `fluzo-docs` revision
  `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: pending. This plan is a working document for an explicitly
  scoped worker run; it does not authorize itself.

## Goal

One review session can cover every merged UI surface at 80x24, 120x40 and
160x50: a deterministic, reproducible sanitized-dump set for each
surface/viewport pair, plus a single checklist that maps every UI-02,
UI-03 and UI-04 acceptance criterion to its evidence pointer and leaves
explicit empty slots for the things only a human can decide.

## Scope and exclusions

In scope:

- A new evidence harness that drives the real `fluzo` binary in a private
  PTY and commits sanitized cell-grid dumps for the UI-03 welcome/setup,
  UI-03 configuration shell and views, UI-02 session/logo/devmenu, and
  the UI-04 task list, task detail, permission, diff and notification
  surfaces, at the three required viewports.
- One review checklist document binding each parent AC of #7, #8 and #9 to
  named tests, artifacts and an evidence class, with human-only slots
  left empty.
- Findings discovered while producing the package, filed as new linked
  GitHub issues.

Excluded:

- No production-code change. A crash-level defect that blocks the package
  would be a separate, clearly labelled minimal commit; none is expected.
- No acceptance of any visual AC. Human slots stay empty by construction.
- No runtime, provider, inference, performance or release claim.
- No changes to the existing `evidence/ui02/` set or its generator.
- No emulator input synthesis, screen-reader run or recording capture;
  those are human-only and are recorded as such.

## Verified context

Observed, not assumed:

- `UI02_ACCEPTANCE_EVIDENCE.md` (merged in #70) already establishes the
  evidence-class vocabulary (`unit`, `buffer`, `pty`, `emulator-input`,
  `artifact`, `human`, `timing`) and the rule that a lower class never
  stands in for a higher one. This package reuses that vocabulary rather
  than inventing a second one.
- `scripts/ui02_evidence.py` is the existing harness: private PTY,
  `--animation-fps 0`, settle-before-dump, hard refusal if any `0x1b`
  byte survives, header with source revision and SHA-256 of the body, and
  a `--verify` mode that requires byte-identical regeneration. The new
  harness reuses `Screen` from `scripts/test_tui.py` the same way.
- Surface reachability, read from `crates/fluzo-tui/src/shell.rs` and the
  CLI help text rather than guessed:
  - UI-03 welcome: run with no `.fluzo` present; marker
    `Welcome to Fluzo`.
  - UI-03 configuration shell: run with a valid `.fluzo`; marker
    `Configuration workspace`.
  - UI-03 configuration views: `Ctrl+P` then the palette query path;
    markers `Providers`, `Limits`, `Theme`.
  - UI-02 session/logo/devmenu: `demo --interactive --dev-menu`; marker
    `Ask anything`, developer menu reached through the developer entry.
  - UI-04 task list: `Ctrl+T` (`Overlay::Tasks`), marker
    `Tasks | offline demo`.
  - UI-04 task detail: `Enter` from the list; marker
    `Task detail | offline demo`.
  - UI-04 permission: `p` from the detail; marker
    `Permission | synthetic request`.
  - UI-04 diff: `d` from the detail; marker
    `Diff & verification | offline demo`.
  - UI-04 notification: developer-menu notification preview; marker
    `Notification test text`.
- Parent issue state: #7 CLOSED, #8 CLOSED, #9 OPEN, #10 OPEN. Filing
  findings must not reopen or alter them beyond a linking comment.
- The repo has no `docs/` tree; durable review documents live at the root
  (`UI02_ACCEPTANCE_EVIDENCE.md`, `FOUNDATION_REVIEW.md`). The checklist
  follows that convention so it sits beside the evidence it describes.

Hypotheses to confirm during the run, not treated as facts:

- Whether every surface actually settles to a static frame at
  `--animation-fps 0`. The UI-02 harness treats a non-settling screen as
  a finding; the same rule applies here and any non-settling surface is a
  finding, not a retry.
- Whether the UI-04 overlays remain legible and non-overflowing at 80x24,
  where the task table has the most columns.

## Decisions, dependencies, and risks

Decisions:

- New harness `scripts/ui05_review_package.py` rather than extending
  `ui02_evidence.py`, so the merged UI-02 artifact set and its
  `--verify` baseline stay untouched and reviewable on their own terms.
- Evidence destination `evidence/ui05/<surface>-<viewport>-<colormode>.txt`
  so surface and viewport are readable from the filename alone.
- Every case pins `--animation-fps 0`. Motion quality is a human slot;
  pinning fps keeps dumps deterministic and makes "settled" meaningful.
- Color modes: truecolor for every surface/viewport pair, plus
  `NO_COLOR` and `high-contrast` for the surfaces where UI-02 already
  proved the reduction path matters. This keeps the set reviewable in one
  sitting instead of 90 files.
- The checklist records evidence class per line item and never marks a
  visual AC accepted.

Dependencies:

- Requires merged #70, #69 and #77 (present at the reviewed revision).
- Requires the `fluzo` binary to build `--locked --offline`.
- No dependency on any open PR.

Risks:

- A surface may not settle at fps 0, or a marker may never appear. Both
  are findings to file, not conditions to work around by loosening the
  harness.
- The sanitized cell model does not track wide-character advance, so
  column alignment in a dump can differ from a real terminal. Dumps are
  for content and presence, never pixel alignment; this limit is carried
  forward from the UI-02 evidence note.
- Fixture configuration must contain no real endpoint or credential. The
  UI-03 fixture uses a bound-and-closed loopback listener exactly as the
  existing S3 PTY tests do, and no request is made.

## Phases

### P1: Deterministic multi-surface evidence set

- Outcome: `evidence/ui05/` contains a sanitized dump for every
  surface/viewport/color-mode case in the matrix, each regenerable
  byte-identically by `--verify`, and the run log records the actual
  result for every case including any failure.
- Owning repository: `fluzo-labs/fluzo`, this branch.
- Dependencies: none beyond the reviewed revision.
- Affected contracts: new test-only script and new committed artifacts. No
  production signature, schema or visible behaviour changes.
- Acceptance criteria:
  - Every case drives the real binary in a private PTY at the stated
    viewport.
  - No dump contains an escape byte; the harness refuses to write one.
  - Every dump header carries the source revision, viewport, color-mode
    environment, exact command and a SHA-256 of the body.
  - `--verify` regenerates and compares; drift is a hard failure.
  - Negative case: a case whose marker never appears aborts rather than
    writing a partial or wrong-screen dump.
- Actions:
  - [x] Write `scripts/ui05_review_package.py` covering the matrix.
  - [x] Run it; record the actual per-case result.
  - [x] Run `--verify`; record whether every artifact is byte-identical.
  - [x] File an issue for any surface that fails to settle or reach its
        marker.
  - [ ] Present evidence and stop for review before the next phase.
- Verification: `python3 -B scripts/ui05_review_package.py` then
  `python3 -B scripts/ui05_review_package.py --verify`, from the
  worktree root, pinned toolchain, `--locked --offline`.
- Risks and recovery: none requiring migration. Regenerating is idempotent
  by design; a wrong artifact is caught by `--verify`.

### P2: Single review checklist with empty human slots

- Outcome: one document a maintainer can work through in a single
  session: every UI-02/UI-03/UI-04 acceptance criterion as a line item,
  each with its evidence pointer and class, plus explicit empty slots for
  Ghostty/Alacritty recordings, screen-reader pass, reduced-motion pass
  and a per-surface verdict.
- Owning repository: `fluzo-labs/fluzo`, this branch.
- Dependencies: P1 (the checklist points at artifacts P1 produces).
- Affected contracts: documentation only.
- Acceptance criteria:
  - Every AC of #7, #8 and #9 appears as a line item; none is dropped or
    merged away.
  - Every line item names at least one concrete evidence pointer (test
    name, artifact path or PR/commit) and exactly one evidence class.
  - No line item is marked accepted; the verdict column is empty.
  - Human-only items are separated and labelled as not achievable by any
    test in this package.
  - The document states the evidence-class rule so a dump is never read
    as visual acceptance.
- Actions:
  - [x] Write the checklist.
  - [x] Cross-check that each cited test actually exists in this tree.
  - [x] Cross-check that each cited artifact path exists.
  - [ ] Present and stop for review.
- Verification: scripted existence check of every cited test name and
  artifact path against the tree; manual read-through.
- Risks and recovery: a stale pointer is the main risk; the scripted
  existence check exists specifically to prevent it.

### P3: Findings filed as linked issues

- Outcome: every defect or gap discovered while producing the package
  exists as a GitHub issue linked to #10, with reproduction steps and the
  evidence pointer, and none of them is fixed in this PR.
- Owning repository: `fluzo-labs/fluzo` issues.
- Dependencies: P1 and P2 (findings come from generating and reading).
- Affected contracts: GitHub issue tracker only.
- Acceptance criteria:
  - Each finding issue states the surface, viewport, color mode, exact
    command, observed versus expected, and the evidence file.
  - Each links to #10 and to its owning parent surface issue.
  - No finding is closed by a code change in this branch.
- Actions:
  - [x] File each discovered finding.
  - [x] Post a summary comment on #10 linking the package and the findings.
  - [ ] Present and stop for review.
- Verification: `gh issue view` on each filed issue; the PR body lists
  them.
- Risks and recovery: duplicate issues are the main risk; search before
  filing.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | 48 artifacts under `evidence/ui05/`; `--verify` reports 48 match, 0 drift |
| P2 | implemented | passed | pending | `review-checklist.md` (same directory); scripted citation check reports no missing test, Python case or artifact path |
| P3 | implemented | passed | pending | #82, #83, #84, #85, #86 filed against `fluzo-labs/fluzo`, each `Ref: #10` |

`last_implementation_at: 2026-10-15T12:01:29Z`

### Phase notes

**P1.** All 48 cases produced evidence; none aborted, so every surface
reached its marker and became static at `--animation-fps 0`. One
non-determinism was found and handled explicitly rather than papered over:
surfaces that render the workspace path made the private temporary root
the only run-varying content, so the generator normalizes it to
`<WORKSPACE_ROOT>` and declares the substitution in each artifact header.
The first `--verify` run before that change reported 18 drifting files,
which is the evidence that the normalization was needed.

**P2.** Two citations were initially wrong and were corrected by the
existence check: a guessed test name
(`crush_shortcuts_respect_context_preserve_demo_authority`, actual name
includes `_and_`) and a wildcard `force_stop_*` replaced with the four
real `tasks::tests` names. No wildcard or invented pointer remains.

**P3.** Five findings, each with reproduction steps, a cause pointer and
artifact paths. None is fixed in this branch. Duplicate search over all
43 existing issues found no overlap before filing.

## Next step

Human review of this package: work through `review-checklist.md` (same directory)
Part C in Ghostty and Alacritty, then fill Part C4. Parts A, B and D are
complete and regenerable. Nothing may be marked accepted by the worker.
Alternative: "stop".

## Evidence and tracking

- Issue: fluzo-labs/fluzo#10 (open). Parents covered: #7 (closed),
  #8 (closed), #9 (open).
- Prior evidence objects reused: `UI02_ACCEPTANCE_EVIDENCE.md` (#70),
  the UI-03 P3 matrix (#69), UI-04 demo states (#77).
- GitHub remains the source of truth for state; this document is a local
  snapshot, not a parallel board.

## Optional handoff to another skill

For an authorized autonomous run, follow the local
[autonomous contract](../../skills/plan-create/references/autonomous.md).
Carry the task-file grant reference, operation allowlist, deadline and
consumed counters with the plan revision and step result. The hard rule
of this task survives handoff: no visual AC may be marked accepted, and
findings are filed, not fixed.
