---
name: "2026-10-15-ui05-stale-tail-rendering"
description: "Fix UI-05 review findings #82, #83 and #85 (stale-cell / clear-region rendering) with regression tests."
created_at: "2026-10-15T13:33:57Z"
has_completed_all_phases: false
---

# Fix stale-tail and clipped-overlay rendering (UI-05 findings #82, #83, #85)

## Source and revision

- Plan identifier: ui05-stale-tail-rendering.
- Source: worker task `/tmp/fluzo-task-T17.md`; GitHub issues
  fluzo-labs/fluzo#82, #83, #85 (filed from the UI-05 review package, PR #87).
- Reviewed revision: `e2bf23a0f33e2b2c3681c1d42124d8b35c3b9e69` (branch
  `fix/ui05-findings-82-83-85`, contains merged UI-04 #77, UI-05 #87, OBS-01 #89).
  Issue `updatedAt`: #82 2026-10-15T12:03:05Z, #83 2026-10-15T12:03:56Z,
  #85 2026-10-15T12:05:34Z.
- Reference repository and branch: `fluzo-labs/fluzo`, `fix/ui05-findings-82-83-85`.
- Design baseline: PRD v0.2 sections 34/53 (MVP scope/acceptance) and the TUI
  rendering conventions already in `crates/fluzo-tui`.
- Approval: delegated autonomous grant in the worker task (analyze, save_plan,
  implement_phase, update_local_progress, commit, push_branch, create_pr,
  continue). Human review/merge remains a human gate. #84 and #86 are explicitly
  out of scope (human design/scope decisions).

## Goal

Replacing the top status line with a shorter string clears the previous string's
tail; the configuration status sentence no longer strands its final period at
80x24; and centred overlay dialogs no longer leave clipped sidebar text flush
against their right border. Each fix is covered by a deterministic regression test
that fails without the change.

## Scope and exclusions

In scope: the top status-row render block, the configuration workspace System
copy, and the wide-mode sidebar render path in `crates/fluzo-tui/src/shell.rs`;
regression tests at 80x24, 120x40 and 160x50.

Excluded: #84 (design decision, human), #86 (scope question, human), the bottom
footer/status bar, the wrap algorithm's general behaviour, any new dependency,
any layout redesign, and any change to overlay content or key handling.

## Verified context

Observed (not hypothesis):

- #82: `render_identity` draws the top status row at
  `Rect::new(area.x + 1, area.y + 2, body_area.width, 1)` only when compact or a
  Developer/Theme overlay is open. It uses `Block::default().style(theme.base)`
  (style only, does not reset cell symbols) plus a short `Paragraph`. The previous
  frame's longer conversation line at that row (e.g. the demo System line
  "Ctrl+P opens commands. Esc stops playback; Ctrl+C exits. No execution.")
  survives the ratatui diff past the short string's end. The activity row at
  `area.y` already uses a per-cell `reset()` + `set_style` loop; the status row
  does not.
- #83: `configuration_shell` appends the System line
  "Configuration workspace. No agent runtime, providers or tools are connected."
  (76 columns). At 80x24 the conversation content width is
  `80 - 2 (body) - 1 (gutter) - 2 (prefix) = 75`, so the trailing "." (a
  separate word bound) wraps alone onto the next row.
- #85: `render_frame` draws the sidebar first, then `render_overlay` draws the
  centred dialog with `Clear` over only the popup rect. In wide mode the dialog's
  right border lands mid-sidebar, so the uncovered sidebar tail (e.g. "ning
  X-High" from "Reasoning X-High") stays visible immediately right of the border.

## Decisions, dependencies, and risks

- #82: apply the existing per-cell clear discipline (reset symbols + base style)
  to the status row before drawing the short string. No per-string padding.
- #83: restructure the copy so it fits the narrowest supported viewport
  (<= 75 columns) rather than change the shared `wrap` algorithm, which is used
  by sidebar names, details and task rows and would risk unrelated layout drift.
  Chosen copy keeps the full meaning.
- #85: while a modal overlay is open in wide mode, clear the sidebar/wordmark
  region instead of drawing it, so no clipped string sits against the border.
  This matches the issue's accepted "sidebar hidden while a modal overlay is open"
  option and keeps the dialog centring unchanged.
- No new dependencies. Risk is limited to TUI presentation; all three are covered
  by buffer-level tests.

## Phases

### P1: Clear-discipline fixes for #82, #83, #85 with regression tests

- Outcome: at 120x40/160x50 a short top status fully replaces a longer prior
  string; at 80x24 the configuration status renders without an orphaned period;
  in wide mode with any modal overlay open, no sidebar glyph appears immediately
  right of the dialog's right border.
- Owning repository: `fluzo-labs/fluzo` (worktree `fluzo-wt-uifix`).
- Dependencies: none beyond the merged baseline.
- Affected contracts: visible rendering only. No protocol, config or storage
  types change.
- Acceptance criteria:
  - #82: rendering a long status then a short status at the top row leaves no
    non-blank cell past the short string across the body width, at 120x40 and
    160x50.
  - #83: the configuration shell at 80x24 has no row whose only non-blank
    content is the trailing period; the sentence occupies a single row.
  - #85: with each of Tasks/Developer/Permission/Diff overlays open in wide
    mode, the cells immediately right of the dialog's right border are blank.
  - Negative: each new test fails against the pre-fix code.
- Actions:
  - [x] #82: replace the `Block::default().style()` + `Paragraph` status block
        with a per-cell reset + base-style loop across the status row, then the
        `Paragraph`.
  - [x] #83: shorten the configuration System copy to <= 75 columns keeping
        meaning.
  - [x] #85: in `render_sidebar`, when a modal overlay is open and not compact,
        clear the sidebar text column and keep the wordmark, so no clipped
        string touches the dialog border.
  - [x] Add Rust buffer regression tests for all three at 80x24, 120x40, 160x50.
  - [x] Run applicable checks and record actual results.
  - [ ] Present evidence and stop for review before merge.
- Verification: pinned toolchain, `--locked --offline`:
  `cargo fmt --all -- --check` (clean); `cargo test --workspace --locked
  --offline` (all green; 179 fluzo-tui lib tests); `cargo clippy --workspace
  --all-targets --locked --offline -- -D warnings` (clean); `python3 -B -m
  unittest discover -s scripts -p 'test_*.py'` (76 OK); `python3
  scripts/check_dev_setup.py` (OK). Negative check: with the three source fixes
  reverted, `shorter_status_line_clears_previous_tail`,
  `configuration_status_does_not_strand_trailing_period` and
  `overlay_dialog_does_not_clip_sidebar_against_right_border` all FAIL while the
  other 176 tui tests pass. Live inference `not_run`.
- Risks and recovery: presentation-only; revert the three edits restores prior
  rendering. No data/migration risk.

Prepared with Fluzo skills
