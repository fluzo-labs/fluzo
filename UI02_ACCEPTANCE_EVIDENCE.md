# UI-02 acceptance evidence (C4)

Requirement-to-implementation-to-test-to-evidence matrix for
[UI-02 (#7)](https://github.com/fluzo-labs/fluzo/issues/7), produced by
[C4 (#39)](https://github.com/fluzo-labs/fluzo/issues/39). This is the durable
evidence object the UI-02 R1 refinement asks for: one place that says which
criterion is covered by which code, which named test, which class of evidence, and
what still needs a human. It does not accept anything.

| Binding | Value |
| --- | --- |
| Source revision | `40577a5f48627474eea53d0b78143c20e226a3ed` (`main`, contains merged C3 #67 and P2 evidence #66) |
| Branch | `feat/ui02-acceptance-evidence` |
| Design baseline | PRD 29.1/29.2.1/29.2.2, PRD 29.4/49 verification boundaries, architecture A06/A07 at `fluzo-docs` `60c5b0732fb710cdf705476cee8d9156a5ecd971`, plus the approved D0 immutable revision |
| Generated | 2026-10-14, pinned toolchain 1.98.0, `--locked --offline` |
| Live inference | `not_run` (no provider listener exists in any fixture used here) |

## Evidence classes

Every row names exactly one class. They are not interchangeable, and a lower class
never stands in for a higher one.

| Class | What it proves | What it does not prove |
| --- | --- | --- |
| `unit` | Pure reducer/policy/clock behaviour on synthetic state and controllable clocks | Any terminal, PTY or emulator behaviour |
| `buffer` | Rendered cell-grid properties (geometry, colors, overlays, cache equivalence) | Visual quality, legibility or contrast on a real display |
| `pty` | Real binary in a real PTY: real input bytes, real `SIGWINCH`, real terminal restore | That a human pressed a key in an emulator, or what a display showed |
| `emulator-input` | Key events delivered through an actual terminal emulator's own input path | Not achieved in this slice; see gaps |
| `artifact` | Committed sanitized cell-grid dump tied to a revision and digest | Color, font rendering or motion as seen on screen |
| `human` | Maintainer visual judgement | Automated reproducibility |
| `timing` | Measured latency in a debug build with observer overhead | PRD 49 release performance acceptance |

## Parent criteria

### 1. Runtime-shaped demo states drive the pixel logo

| Implementation | Tests | Class | Status |
| --- | --- | --- | --- |
| `crates/fluzo-tui/src/identity.rs` (state tints, relief geometry, compact sampling, wave, 80% bar, void/fallbacks) | `state_tints_preserve_relief_geometry_and_static_fallbacks`, `compact_logo_samples_the_large_logo_palette_and_phase`, `working_label_cycles_color_without_changing_text_or_disabled_motion`, `message_wave_has_fixed_width_elapsed_motion_and_static_fallbacks`, `eighty_percent_bar_reaches_both_edges_without_overflow`, `idle_and_disabled_motion_are_static`, `transparent_void_and_terminal_fallbacks_preserve_shape` | unit + buffer | covered |
| `crates/fluzo-tui/src/visual.rs` (`motion_active`, `tick`) | `artwork_uses_elapsed_time_not_frame_count`, `waiting_ticks_and_completion_settles_without_an_idle_loop` | unit | covered |
| `crates/fluzo-tui/src/shell.rs` (wordmark placement, no overpaint) | `relief_wordmark_fits_sidebar_and_honors_motion_preferences`, `visual_menu_does_not_overpaint_any_wordmark_row` | buffer | covered |
| Committed dumps show the wordmark in compact (80x24) and wide sidebar (120x40, 160x50) form | `evidence/ui02/session-{80x24,120x40,160x50}-{truecolor,ansi,nocolor}.txt`, `evidence/ui02/session-*-high-contrast.txt` | artifact | covered |

Boundary: the states are **synthetic demo states**, deliberately runtime-shaped.
They are not driven by a real task snapshot. Real-runtime logo behaviour belongs to
[#24](https://github.com/fluzo-labs/fluzo/issues/24) and is not claimed here.

### 2. 60/30/15/0 FPS and reduced motion work

| Implementation | Tests | Class | Status |
| --- | --- | --- | --- |
| `visual.rs` clock and cap logic; `safe_screen_settings` override | `clock_caps_animation_skips_backlog_and_never_ticks_idle`, `artwork_uses_elapsed_time_not_frame_count`, `safe_screen_settings_override_visual_flags_without_disabling_devmenu` | unit (controllable clock) | covered |
| `identity.rs` static paths | `idle_and_disabled_motion_are_static` | unit + buffer | covered |
| FPS 0 interactivity and read-only previews | `scripts/test_tui.py::test_zero_fps_developer_previews_remain_interactive_and_read_only` | pty | covered |
| Animation during resize and control | `scripts/test_tui.py::test_active_animation_resize_and_controls_remain_safe` | pty | covered |
| Static-frame proof | `scripts/ui02_evidence.py` refuses to emit an artifact until two consecutive screen reads are identical, so every committed dump is direct evidence that the `--animation-fps 0` presentation reaches a static frame | pty + artifact | covered |

Boundary: physical display cadence and frame-pacing percentiles are **not**
measured here. That gate is [#26](https://github.com/fluzo-labs/fluzo/issues/26).

### 3. Previews revert and no visual flag grants runtime authority

| Implementation | Tests | Class | Status |
| --- | --- | --- | --- |
| `visual.rs` preview/apply/reset with authority checks | `previews_are_reversible_and_cannot_change_authority`, `cli_locks_and_invalid_values_are_enforced` | unit | covered |
| `shell.rs` revert on every exit, scoped reset, demo authority | `visual_previews_preserve_navigation_and_revert_on_every_exit`, `theme_reset_preserves_other_preferences_and_remains_reversible`, `crush_shortcuts_respect_context_and_preserve_demo_authority`, `non_demo_snapshots_and_incompatible_versions_are_rejected` | buffer | covered |
| `configuration.rs` authority/draft preservation on preview and exit prompt | `theme_preview_and_exit_prompt_preserve_authority_and_drafts`, `mvp_developer_entry_opens_real_presentation_settings_without_demo_actions` | buffer | covered |
| Developer-menu gating | `workspace.rs::developer_menu_entry_is_hidden_when_developer_preferences_are_disabled`, `palette_preserves_shared_dialog_design_and_selected_row_after_resize` | buffer | covered |
| C3 shared save/apply route (no second writer, CLI locks win, apply is the only path to effective change) | `crates/fluzo-cli/tests/configuration.rs` route tests (10) | pty-free integration over the real service | covered |
| Terminal never gains authority from presentation state | `terminal.rs::restoration_attempts_all_modes_despite_write_failure`, `scripts/test_tui.py::test_noninteractive_invocation_fails_without_escape_output` | unit + pty | covered |

### 4. Evidence and relevant documentation are linked

Answered by this file, the pointer section in `TUI.md`, the committed artifacts
under `evidence/ui02/`, and the delivery PR reference to #39.

## R1 contract rows

### Controls (C1)

| Contract | Tests | Class | Status |
| --- | --- | --- | --- |
| Four typed visual preferences only; invalid values rejected | `visual.rs::cli_locks_and_invalid_values_are_enforced`, `lib.rs::configuration_controls_share_core_defaults_and_validation`, `lib.rs::visual_metadata_and_direct_validation_enforce_the_same_choices` | unit | covered |
| Theme-only reset; developer-menu reset scoped to the four keys | `shell.rs::theme_reset_preserves_other_preferences_and_remains_reversible` | buffer | covered |
| Lock feedback follows the selected entry | `shell.rs::visual_lock_feedback_tracks_selection_instead_of_last_error`, `desktop_preference_metadata_distinguishes_apply_capability_and_cli_lock` | buffer | covered |
| Navigation keys, held repeat, search interplay | `shell.rs::developer_navigation_and_notification_editor_preserve_composer`, `developer_menu_is_conditional_searchable_and_bounded` | buffer | covered |

### Preview / persistence / isolation (C2, C3)

| Contract | Tests | Class | Status |
| --- | --- | --- | --- |
| Notification preview is bounded, sanitized, never persisted | `notification.rs::editable_test_message_is_bounded_sanitized_and_snapshotted`, `notification_stack.rs::text_is_sanitized_bounded_and_truncated_at_grapheme_boundaries` | unit | covered |
| Desktop delivery needs opt-in, supported transport and observed blur | `notification.rs::completion_requires_opt_in_supported_transport_and_observed_blur`, `notification_opt_in_preserves_disabled_default`, `shell.rs::desktop_preference_requires_supported_transport_and_explicit_apply` | unit + buffer | covered |
| No replay after suppression, disable or failure | `test_deadline_is_bounded_replaced_and_consumed_once_even_when_suppressed`, `disabling_cancels_pending_delivery_and_reenable_does_not_replay`, `failed_notification_is_not_retried` | unit | covered |
| Bounded stack, priority-preserving overflow, no unbounded dedup | `notification_stack.rs::overflow_keeps_priority_then_age_without_unbounded_dedup_storage`, `oldest_information_is_discarded_before_older_errors`, `duplicates_never_extend_expiry_or_resurrect_removed_notices` | unit | covered |
| Notification settings cannot be changed by notice arrival | `pending_notices_expire_from_receipt_and_capacity_never_changes_settings`, `duration_and_visibility_boundaries_apply_without_rewriting_configuration` | unit | covered |
| Save/apply goes only through the shared typed service; saved-future vs active-effective | C3 route tests in `crates/fluzo-cli/tests/configuration.rs` | integration over the real service | covered |
| Failed/uncertain save never claims success | `configuration.rs::read_only_capacity_conflict_and_unknown_keep_drafts`, `failures_and_closed_view_completion_are_not_replayed` | buffer | covered |
| No notice wire without opt-in | `scripts/test_tui.py::test_notification_focus_policy_and_wire_output_without_desktop_access` (asserts no OSC 777 wire) | pty | covered |

### Keyboard-only, paste, focus/draft/scroll, resize, color modes

| Contract | Tests | Class | Status |
| --- | --- | --- | --- |
| Keyboard-only operation across surfaces | `shell.rs::tab_focus_selects_latest_item_dims_editor_and_restores_draft`, `crush_chat_navigation_and_editor_history_have_distinct_keys`, `palette_search_actions_and_cancel_never_discard_input`, `help_table_aligns_columns_without_rules_and_pages_all_shortcuts` | buffer | covered |
| Safe paste never submits; bracketed paste with embedded OSC is sanitized | `shell.rs::paste_never_submits_and_overlays_preserve_draft_focus_and_scroll`; `scripts/test_tui.py` paste fixture (`\x1b[200~ … \x1b]52;c;hidden\x07 … \x1b[201~`) | buffer + pty | covered |
| Focus, draft and scroll survive activity, overlays and resize | `unread_activity_is_visible_until_follow_without_changing_focus`, `wheel_scroll_is_viewport_scoped_bounded_and_preserves_input`, `wrapped_messages_keep_selection_scroll_and_complete_text_across_resize` | buffer | covered |
| Resize during activity | `scripts/test_tui.py::test_active_animation_resize_and_controls_remain_safe`; `shell.rs::animation_geometry_refreshes_after_reflow_scroll_and_expansion` | pty + buffer | covered |
| Viewports 80x24, 120x40, 160x50 | 46 / 54 / 28 buffer-test size occurrences across `crates/fluzo-tui/src` (also 60x16 x35, 196x36 and 40x10); PTY loops at 24x80, 16x60, 40x120, 50x160, 240x80; committed dumps at all three | buffer + pty + artifact | covered |
| Unsupported size is explicit, not corrupt | PTY resize to 8x30 asserts the `Minimum: 60x16` notice; `shell.rs::renders_supported_viewports_and_small_notice_without_controls` | pty + buffer | covered |
| truecolor / ANSI / NO_COLOR | `visual.rs::ansi_reduction_preserves_visible_rules_and_is_idempotent`, `shell.rs::capability_fallback_covers_every_rendered_surface`, `high_contrast_fire_covers_identity_loaders_and_cached_frames`; PTY env variants; 9 committed color-mode dumps | buffer + pty + artifact | covered |
| Terminal restoration after normal and abnormal exit | `scripts/test_tui.py::test_external_termination_restores_terminal`, `terminal.rs::restoration_attempts_all_modes_despite_write_failure` | pty + unit | covered |

## Committed sanitized artifacts

Generated by `python3 -B scripts/ui02_evidence.py`; reproduced with
`python3 -B scripts/ui02_evidence.py --verify` (15/15 byte-identical at this
revision). Each file header carries the source revision, viewport, color-mode
environment, exact command and a SHA-256 of the body.

| Set | Files | Class |
| --- | --- | --- |
| Session view, 3 viewports x truecolor/ANSI/NO_COLOR | `session-{80x24,120x40,160x50}-{truecolor,ansi,nocolor}.txt` | artifact |
| High-contrast theme at the three viewports | `session-{80x24,120x40,160x50}-high-contrast.txt` | artifact |
| Help, sessions and models dialogs at 120x40 | `help-120x40-truecolor.txt`, `sessions-120x40-truecolor.txt`, `models-120x40-truecolor.txt` | artifact |

Why these are safe to commit and raw captures are not: the harness renders the
stream through the shared `Screen` cell model from `scripts/test_tui.py` and
aborts if any `0x1b` byte survives, so no terminal-control sequence can reach the
repository. All content is synthetic demo content; no user text, endpoint or
credential can appear because the demo never reads user configuration.

Artifact limitation: the cell model does not track wide-character advance, so
column alignment in a dump can differ from what a terminal paints. Use dumps for
content and presence review, never for pixel alignment. Contrast and legibility
still require the human review below.

## Terminal environment records

Both target emulators are installed on the evidence machine and were checked
directly rather than cited from memory.

| Item | Value |
| --- | --- |
| Session | `XDG_SESSION_TYPE=wayland`, `XDG_CURRENT_DESKTOP=KDE` |
| Compositor | `kwin 6.7.5` (`kwin_wayland`) |
| Kernel / distro | `7.2.9-2-cachyos` |
| GPU 1 | NVIDIA GA104M GeForce RTX 3070 Mobile / Max-Q `[10de:249d]`, driver `nvidia` |
| GPU 2 | AMD Cezanne Radeon Vega `[1002:1638]`, driver `amdgpu` |
| Ghostty | `Ghostty 1.3.1-arch2.2` |
| Alacritty | `alacritty 0.17.0 (94e7c887)`, user config present at `~/.config/alacritty/alacritty.toml` |
| Toolchain | rust 1.98.0 pinned by `rust-toolchain.toml`; Python 3 with `unittest` |

Prior accepted UI-02 relay probes ran in Ghostty 1.3.1-arch2.2 and Alacritty
0.17.0 under X11 with temporary `HOME`/configuration. Those probes injected input
into an inner PTY. The environment above is the current Wayland machine; the
emulator-input class has still not been reached on either terminal.

## Needs human review

Nothing in this section can be closed by a test. Each item is a maintainer action.

1. **Actual emulator keyboard input.** No key event has ever gone through
   Ghostty's or Alacritty's own input path in this evidence set. On this machine
   there is no Wayland input-synthesis tool available (`ydotool`, `kdotool` and
   `wlrctl` are absent; `xdotool` cannot reach Wayland-native windows; KWin D-Bus
   scripting loads but does not execute). A human should drive the demo with real
   keys in both terminals at 80x24, 120x40 and 160x50: composer editing,
   bracketed paste, Ctrl+P palette, Ctrl+G help, Ctrl+B sidebar, Esc cancel,
   Ctrl+C exit.
2. **Legibility and contrast.** Review the committed dumps for content, then the
   live rendering for contrast. The known subdued default-theme secondary model and
   footer text flagged in earlier Alacritty captures has **not** been re-reviewed
   and is not resolved by the ANSI/NO_COLOR tests.
3. **Motion quality.** Whether the wave, pulse and loader read as intended at
   60/30/15 FPS on a real display. Automated tests prove the clock, not the feel.
4. **Notification behaviour.** Ghostty desktop delivery was previously confirmed
   by the maintainer; that confirms arrival, not popup visibility, and does not
   cover the original missing-popup report. Alacritty has no notification adapter
   and is not counted as supported.
5. **High-contrast palette on both terminals.** Buffer tests cover coverage and
   idempotence; perceived contrast of the fire palette against black, and the
   semantic state tints for success/waiting/error/cancellation, need eyes.
6. **Image or recording evidence.** Only sanitized cell grids are committed. If
   the acceptance gate needs images or a recording, they must come from a human
   capture or a CI artifact store; neither exists for this slice.

## Preserved failed attempts

Kept as evidence history, not reported as passes.

- First artifact run: 12 of 15 cases wrote, then `help-120x40-truecolor` failed
  with "marker never appeared". Cause: the harness waited for the *view* marker
  before sending the view key. Fixed by splitting the startup marker from the view
  marker.
- Second artifact run: all 15 wrote, but `--verify` reported drift on
  `session-160x50-truecolor`, `session-160x50-ansi` and
  `sessions-120x40-truecolor`. Diffing showed the captured frames were taken
  mid-transition (dialog wave still expanding, footer still re-laying out). Fixed
  by requiring the screen to stop changing before dumping, which also turned the
  static-frame property into directly observed evidence.
- Environment failure during this slice: the worker's shell working directory was
  removed by a parallel cleanup while this session was still pointed at it, which
  blocked all command execution. Recovered by recreating the path. Not an
  application issue; recorded because it interrupted the verification sequence.
- Earlier accepted UI-02 probes preserved their own failures (stale screen markers,
  wrong assumed editor row, initial Ghostty cell sizes, blocking PTY writes, a
  4 MiB capture cap, a broad single-character marker). Those remain recorded in
  `TUI.md` and are not re-run or re-claimed here.

## Exploratory timing findings (not PRD 49 results)

From the earlier debug-build probe retained by the UI-02 stabilization, on a
debug build with Python observer overhead:

| FPS | Reduced motion | Median ms | p95 ms | Max ms |
| --- | --- | --- | --- | --- |
| 0 | off | 29.524 | 35.941 | 36.369 |
| 15 | off | 30.409 | 40.660 | 61.856 |
| 30 | off | 33.476 | **52.648** | 63.345 |
| 60 | off | 33.255 | 40.338 | 65.864 |
| 60 | on | 29.510 | 35.086 | 35.380 |

Eight-character bursts reached p95 **292-302 ms**. These stay visible as
findings. They are not PRD 49 acceptance, not a waiver, and imply no default-rate
reduction: the release build, approved hardware profile, 1000-sample plan,
high-rate event workload and persisted large-history fixture that PRD 49 needs
were not supplied. The gate remains
[#26](https://github.com/fluzo-labs/fluzo/issues/26).

No new timing measurement was taken in this slice, so no new timing claim is made.

## Gaps and owning issues

| Gap | Class blocked | Owning issue | Status here |
| --- | --- | --- | --- |
| Real emulator keyboard path in Ghostty/Alacritty | `emulator-input` | needs a human; tracked under this C4 checklist item 4 | open, tooling absent on this machine |
| Visual legibility, contrast, motion feel, subdued secondary text | `human` | maintainer review; cross-view review is [#10](https://github.com/fluzo-labs/fluzo/issues/10) | open |
| Physical display latency and PRD 49 performance | `timing` | [#26](https://github.com/fluzo-labs/fluzo/issues/26) | open, out of C4 scope |
| Logo/state driven by a real task snapshot | — | [#24](https://github.com/fluzo-labs/fluzo/issues/24) | open, out of C4 scope |
| Durable hosted screenshots/recordings | `artifact` (image) | needs CI artifact store or human capture | open; sanitized text dumps committed instead |
| Notification popup visibility | `human` | maintainer; adapter absence is a known Ghostty-only limitation | open |

No gap in this table is closed by this slice. Mandatory UI-02 gaps still block
parent closure even though owners exist.

## Verification actually run

Pinned toolchain 1.98.0, `--locked --offline`, from the worktree root.

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo test --workspace --locked --offline` | pass — 272 tests (150 TUI, 78 runtime, 16 core, 12 HTTP-simulator, 14 CLI) |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | pass, no warnings |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | pass — 74 tests in 103.3 s (includes LSP, HTTP namespace profile and PTY suite) |
| `python3 scripts/check_dev_setup.py` | pass — graph, toolchain, LSP config and 12 skills |
| `python3 -B scripts/ui02_evidence.py` | pass — 15 artifacts written |
| `python3 -B scripts/ui02_evidence.py --verify` | pass — 15/15 byte-identical |
| Live inference | `not_run` |
| Remote CI run | not implied; no remote workflow run is claimed |

## Boundaries

This matrix does not claim UI-02 parent acceptance, runtime integration (#24),
release or reference performance (#26), cross-view visual review (#10), or any
issue closure. It records what is covered, at which evidence class, and exactly
what a human still has to look at.
