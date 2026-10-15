# UI-05 review checklist

Single-session review package for every merged UI surface. Issue:
[fluzo-labs/fluzo#10](https://github.com/fluzo-labs/fluzo/issues/10).
Surfaces reviewed: UI-02 ([#7](https://github.com/fluzo-labs/fluzo/issues/7)),
UI-03 ([#8](https://github.com/fluzo-labs/fluzo/issues/8)) and UI-04
([#9](https://github.com/fluzo-labs/fluzo/issues/9)).

Source revision of the evidence: `b0448790ee480f57f527239c6041c906034e72d0`.

## How to read this document

Every line item carries an **evidence class**. A lower class never stands in
for a higher one, and no class in this package equals acceptance.

| Class | What it proves | What it does not prove |
| --- | --- | --- |
| `unit` | A pure reducer or policy decision in isolation | That anything rendered |
| `buffer` | Ratatui buffer contents for a constructed state | Real terminal encoding, real timing |
| `pty` | The real binary rendered into a real pseudo-terminal at a pinned size | Visual quality, colour accuracy, motion, accessibility, human acceptance |
| `emulator-input` | Key bytes survive a real terminal's input path | What the user saw |
| `artifact` | A committed, regenerable dump | Anything about how it looked |
| `human` | A person looked at it and decided | Nothing automatic; must be recorded by that person |
| `timing` | A measured rate under a controlled run | Physical display cadence on an unmeasured machine |

The `pty` dumps under `evidence/ui05/` are **sanitized cell grids**. The
generator interprets every escape sequence through the shared `Screen`
renderer and refuses to write a dump containing a surviving `0x1b` byte.
A dump therefore shows which glyph occupied which cell. It does not carry
colour, font, sub-cell positioning or wide-character advance, so column
alignment in a dump can differ from a real terminal. Use dumps for content
and presence; use the human slots for everything visual.

**Nothing in this document is marked accepted.** The verdict columns are
empty by construction and stay empty until a person fills them.

## Regenerating the evidence

```sh
python3 -B scripts/ui05_review_package.py            # rewrite evidence/ui05/
python3 -B scripts/ui05_review_package.py --verify   # regenerate and compare
python3 -B scripts/ui05_review_package.py --list     # print the case matrix
```

`--verify` currently reports `match` for all 48 artifacts. Every case pins
`--animation-fps 0`, so no frame timing enters an artifact. A case whose
marker never appears, or whose screen never becomes static, aborts rather
than writing a partial or wrong-screen dump.

Two normalizations are declared in each artifact header rather than hidden:
the private temporary workspace root is rendered as `<WORKSPACE_ROOT>`
(it is the only run-varying content on surfaces that show the config
path), and trailing whitespace is stripped per line.

## Part A: Evidence matrix

48 artifacts under `evidence/ui05/`, named
`<surface>-<rows>x<cols>-<color-mode>.txt`.

| Surface | Story | 80x24 | 120x40 | 160x50 |
| --- | --- | --- | --- | --- |
| Welcome / create-configuration dialog | UI-03 | `welcome-24x80-{truecolor,nocolor,high-contrast}` | `welcome-40x120-{...}` | `welcome-50x160-{...}` |
| Configuration workspace (base shell) | UI-03 | `config-workspace-24x80-{truecolor,nocolor,high-contrast}` | `config-workspace-40x120-{...}` | `config-workspace-50x160-{...}` |
| Configuration command palette (`Ctrl+P`) | UI-03 | `config-palette-24x80-truecolor` | `config-palette-40x120-truecolor` | `config-palette-50x160-truecolor` |
| Demo session, wordmark and sidebar | UI-02 | `session-24x80-{truecolor,nocolor,high-contrast}` | `session-40x120-{...}` | `session-50x160-{...}` |
| Developer menu | UI-02 | `devmenu-24x80-truecolor` | `devmenu-40x120-truecolor` | `devmenu-50x160-truecolor` |
| Task list | UI-04 | `tasks-24x80-truecolor` | `tasks-40x120-truecolor` | `tasks-50x160-truecolor` |
| Task detail | UI-04 | `task-detail-24x80-truecolor` | `task-detail-40x120-truecolor` | `task-detail-50x160-truecolor` |
| Permission prompt | UI-04 | `permission-24x80-truecolor` | `permission-40x120-truecolor` | `permission-50x160-truecolor` |
| Diff and verification | UI-04 | `diff-24x80-truecolor` | `diff-40x120-truecolor` | `diff-50x160-truecolor` |
| Notification stack with notices | UI-04 | `notices-24x80-truecolor` | `notices-40x120-truecolor` | `notices-50x160-truecolor` |

Colour-mode coverage is complete for the four surfaces where UI-02 and
UI-03 already established that the reduction path matters (welcome,
configuration workspace, session) and single-mode for the overlay
surfaces, whose content is colour-independent. The full 90-cell matrix is
one edit to `SURFACES` in the generator away if a reviewer wants it.

## Part B: Acceptance-criterion mapping

### UI-02 (#7) — logo, themes, animation, devmenu

| Acceptance criterion | Evidence pointer | Class | Human verdict |
| --- | --- | --- | --- |
| Runtime-shaped demo states drive the pixel logo | `session-*-truecolor` (wide sidebar relief wordmark); `session-24x80-*` (compact one-line header); `shell::tests::relief_wordmark_fits_sidebar_and_honors_motion_preferences` | pty + buffer | ☐ |
| 60/30/15/0 FPS and reduced motion work | Every case pins `--animation-fps 0`; `shell::tests::loading_timer_tracks_playback_across_fragments_and_disabled_motion`; UI-02 FPS matrix in `UI02_ACCEPTANCE_EVIDENCE.md` | buffer + timing (UI-02 only) | ☐ |
| Previews revert and no visual flag grants runtime authority | `shell::tests::visual_previews_preserve_navigation_and_revert_on_every_exit`; `shell::tests::crush_shortcuts_respect_context_and_preserve_demo_authority`; `shell::tests::notification_preview_is_explicit_bounded_and_preserves_user_state` | unit + buffer | ☐ |
| Evidence and documentation linked from the completing PR | This document, `evidence/ui05/`, `UI02_ACCEPTANCE_EVIDENCE.md`, the PR body | artifact | ☐ |

### UI-03 (#8) — welcome and typed configuration

| Acceptance criterion | Evidence pointer | Class | Human verdict |
| --- | --- | --- | --- |
| Missing config opens setup | `welcome-{24x80,40x120,50x160}-*`; `scripts/test_setup.py::test_setup_save_requires_two_explicit_steps` | pty | ☐ |
| Existing valid config skips it | `config-workspace-*` (reached with a valid `.fluzo`, no welcome shown); `scripts/test_setup.py::test_valid_configuration_skips_setup` | pty | ☐ |
| Temporary-fixture save/reload, defaults, conflicts and CLI overrides tested without contacting models | `configuration_tests.rs::review_regression_effective_only_collections_can_be_recreated_after_reload`, `::read_only_capacity_conflict_and_unknown_keep_drafts`, `::restore_defaults_targets_the_explicit_selection_and_stays_in_the_draft`; `scripts/test_configuration.py::test_normal_settings_apply_cli_locks_resize_and_preserve_files`, `::test_outside_change_banner_offers_keep_ours_then_reload`; `scripts/test_setup.py::test_offline_provider_is_not_contacted_or_resolved` | unit + pty | ☐ |
| Evidence and documentation linked from the completing PR | This document, `CONFIGURATION.md`, the PR body | artifact | ☐ |

### UI-04 (#9) — tasks, permissions, diff, notifications

| Acceptance criterion | Evidence pointer | Class | Human verdict |
| --- | --- | --- | --- |
| Selecting a task does not resume it | `task-detail-*` (row: "Selecting a task never resumes it: no writer ownership, no grant change, no input redirect"); `shell::tests::opening_tasks_does_not_dispatch_and_records_no_side_effect` | pty + unit | ☐ |
| Cancel and kill controls are distinct | `task-detail-*` footer (`c cancel \| f force stop`); `tasks::tests::cancel_and_force_stop_are_distinct_controls`, `tasks::tests::force_stop_requires_an_explicit_move_to_the_destructive_row`, `tasks::tests::force_stop_always_passes_through_the_confirmation` and `tasks::tests::repeated_cancellation_is_idempotent_and_never_escalates` | pty + unit | ☐ |
| Permission focus is safe | `permission-*` (Deny is the default and pre-selected; "Recording is not dispatch: no grant is consumed"); `shell::tests::notification_close_click_targets_one_notice_without_changing_focus` for focus discipline | pty + buffer | ☐ |
| Content is sanitized | `shell::tests::sanitizer_handles_split_sequences_and_bidi_without_buffering`, `shell::tests::task_notice_text_is_sanitized`; generator refuses any dump containing `0x1b` | unit + pty | ☐ |
| Every simulated result is visibly synthetic | `tasks-*`, `task-detail-*`, `permission-*`, `diff-*`, `notices-*` each carry a `synthetic`/`offline demo` label in the title bar or body | pty | ☐ |
| Evidence and documentation linked from the completing PR | This document, `TUI.md`, the PR body | artifact | ☐ |

## Part C: Human-only slots

None of these can be produced by any test in this package. They are the
reason this document exists: so a reviewer knows exactly what is still
missing rather than assuming a green suite means a green UI.

### C1 — Ghostty

| Item | 80x24 | 120x40 | 160x50 | Notes |
| --- | --- | --- | --- | --- |
| Recording of the full surface walk | ☐ not captured | ☐ | ☐ | |
| Wordmark legibility and spacing | ☐ | ☐ | ☐ | |
| Colour and contrast as actually rendered | ☐ | ☐ | ☐ | |
| Loader behaviour under work/idle | ☐ | ☐ | ☐ | |
| Overlay placement against the sidebar | ☐ | ☐ | ☐ | see #85 |

### C2 — Alacritty

| Item | 80x24 | 120x40 | 160x50 | Notes |
| --- | --- | --- | --- | --- |
| Recording of the full surface walk | ☐ not captured | ☐ | ☐ | |
| Unicode/font fallback for the wordmark and box drawing | ☐ | ☐ | ☐ | |
| Overlay placement against the sidebar | ☐ | ☐ | ☐ | see #85 |

### C3 — Accessibility

| Item | Status | Notes |
| --- | --- | --- |
| Screen-reader pass over every surface | ☐ not run | No automated substitute exists |
| Reduced-motion pass by eye | ☐ not run | Dumps are pinned at fps 0 and cannot show motion |
| `NO_COLOR` and high-contrast by eye | ☐ not run | Dumps strip colour entirely |
| Keyboard-only reachability of every action | ☐ not run | Partially covered by unit tests, not by a human pass |

### C4 — Maintainer verdict

| Surface | Verdict | Reviewer | Date | Notes |
| --- | --- | --- | --- | --- |
| Welcome / create configuration | ☐ accept ☐ changes | | | |
| Configuration workspace and palette | ☐ accept ☐ changes | | | |
| Demo session, wordmark, sidebar | ☐ accept ☐ changes | | | |
| Developer menu | ☐ accept ☐ changes | | | |
| Task list and detail | ☐ accept ☐ changes | | | |
| Permission prompt | ☐ accept ☐ changes | | | |
| Diff and verification | ☐ accept ☐ changes | ☐ | ☐ | |
| Notification stack | ☐ accept ☐ changes | | | |

Jose's overall verdict: ☐ **pending** — no part of UI-05 is accepted by
this package.

## Part D: Findings filed from this package

Discovered while generating and reading the dumps. Each is filed as its own
issue and **none is fixed in this branch**.

| Issue | Finding | Severity | Where visible |
| --- | --- | --- | --- |
| [#82](https://github.com/fluzo-labs/fluzo/issues/82) | Shorter status line leaves the previous string's tail visible (`offlineyback; Ctrl+C exits.`) | Bug, medium | `devmenu-40x120-truecolor`, `devmenu-50x160-truecolor` row 2; reproduced in tmux independently |
| [#83](https://github.com/fluzo-labs/fluzo/issues/83) | Configuration status line strands the final period at 80x24 | Bug, cosmetic | `config-workspace-24x80-*` rows 3-4 |
| [#84](https://github.com/fluzo-labs/fluzo/issues/84) | Notification card placement asymmetric and viewport-inconsistent (left margin 30/38/78 vs right 2/34/34; never centred) | Design decision | `notices-*-truecolor` |
| [#85](https://github.com/fluzo-labs/fluzo/issues/85) | Overlay dialogs clip sidebar text against their right border (`╮ithub Copilot`, `│ning X-High`) | Bug, cosmetic | every `tasks-*`, `devmenu-*`, `permission-*`, `diff-*` at 120x40 and 160x50 |
| [#86](https://github.com/fluzo-labs/fluzo/issues/86) | Configuration palette exposes an unimplemented `Plugins` entry | Scope question | `config-palette-*-truecolor` |

## Part E: Not covered by this package

Stated so a reviewer does not infer coverage that is not there.

- **No visual acceptance.** Part C is empty and only a person can fill it.
- **No motion evidence.** Every dump is pinned at `--animation-fps 0`.
  Loader cadence, animation smoothness and reduced-motion behaviour are
  unproven here; the UI-02 `timing` evidence in `UI02_ACCEPTANCE_EVIDENCE.md`
  is the only rate evidence that exists, and it is not a physical-display claim.
- **No colour evidence.** Dumps are cell grids. `nocolor` and
  `high-contrast` cases prove the code path runs and produces the same
  layout, not that it looks right.
- **No Ghostty or Alacritty emulator run.** The PTY here is a plain
  pseudo-terminal driven through the shared `Screen` renderer.
- **No screen-reader or accessibility tooling.**
- **No real runtime, provider, inference or task execution.** Every
  surface is synthetic demo or configuration-only state, which is what
  those stories delivered.
- **No performance measurement.** No frame-rate, latency or throughput
  number is claimed.
- **No Windows or macOS rendering.** Linux only.
- **The UI-04 task list contains one synthetic task.** Multi-state visual
  coverage (running, completed, failed, cancelled side by side) is not in
  the dumps because the fixture does not produce it.

## Verification commands and results

Run at the source revision above, pinned toolchain, `--locked --offline`.

| Command | Result |
| --- | --- |
| `python3 -B scripts/ui05_review_package.py` | 48 artifacts written |
| `python3 -B scripts/ui05_review_package.py --verify` | 48 match, 0 drift |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | pass |
| `cargo test --workspace --locked --offline` | pass, 0 failures |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | 76 pass |
| `python3 scripts/check_dev_setup.py` | pass |
