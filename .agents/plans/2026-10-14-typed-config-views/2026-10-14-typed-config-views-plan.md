---
name: "2026-10-14-typed-config-views"
description: "Basic/advanced tiers come from the shared typed registry, and Restore defaults acts on an explicit selection in the draft only, so the normal configuration editor satisfies the UI-03 typed-editing criterion without a form engine or hardcoded field lists."
created_at: "2026-10-14T11:23:48Z"
has_completed_all_phases: false
---

# Typed configuration views for the normal configuration shell

## Source and revision

- Plan identifier: `typed-config-views` (UI-03 R1 / S3 continuation).
- Source: [fluzo-labs/fluzo#46](https://github.com/fluzo-labs/fluzo/issues/46),
  "Add typed configuration views and verify UI-03 acceptance", including
  "Refinement R2: normal configuration integration and acceptance".
- Reviewed revision: issue `updatedAt 2026-10-08T10:29:50Z`; local branch
  `feat/typed-config-views` created from `main` at
  `ffabfea4a2d2ad5bf42dbb373e217950df32ca50`.
- Reference repository and branch: `fluzo-labs/fluzo`, `main`. The issue's own
  implementation baseline is `d03b60d84e7dcd225348baf2fe8888cbd22640d9`;
  `ffabfea` is a descendant that already contains the S1/S2/S3 surfaces this plan
  builds on.
- Design baseline: PRD 26 / 29.2.2 / 30 and architecture A06 / A07 sections 5.1
  and 10 under D0 `53b345e9f1782beef839b42b4c0de4773ed371df`, plus the
  approved configuration-write boundary from fluzo-docs#11 revision
  `ed08afab9e2e3ac22a9ef5ef89e32911fcda1850` (PRD 26.1, architecture
  10.2/10.3). Consulted through the issue text and `AGENTS.md`; the documents
  themselves were not re-fetched in this session.
- Approval: the task instruction explicitly authorizes creating this plan at this
  path and implementing P1 only. This field is an annotation, not proof of
  authorization.

## Goal

A person in the normal configuration shell sees basic settings by default and
reaches advanced settings with one key, with the split owned by the shared typed
registry rather than a hardcoded field list, and can reset exactly the settings
they selected back to their registry defaults in the draft without touching the
file, without deleting a collection and without saving.

## Scope and exclusions

In scope (P1):

- A `SettingTier` (`Basic` / `Advanced`) carried by every `SettingDescriptor`,
  assigned in the registry next to privacy and application rule, so the tier of a
  collection member and its template cannot drift apart.
- Configuration protocol 6, so the tier travels to the TUI inside the owned
  descriptor it already owns, and the runtime rejects the previous protocol.
- The configuration editor hides advanced-tier fields until `F4`; `F4` toggles
  back. Filter, selection, checked set, drafts and pending requests survive the
  toggle.
- `Ctrl+D` restores the registry default for the explicit selection (Space-checked
  keys) or, when nothing is checked, for the focused key. Draft only: no port
  request, no file write, no collection removal.
- Documentation of the tier metadata, the `F4` route and the selection-scoped
  restore in `CONFIGURATION.md`, `TUI.md`, `APPLICATION.md` and `AGENTS.md`.

Excluded from P1 (deliberate):

- The S2 setup wizard's hardcoded ten-field basic view (`setup.rs`) is not
  re-tiered. It is accepted S2 behavior with its own visual gate; aligning it with
  registry tiers is a separate deliverable so this change stays reviewable.
- No new settings, no new defaults, no new namespaces, no palette entries.
- No arbitrary keys, no executable expressions, no raw TOML editor and no generic
  form engine. The tier is metadata on an existing descriptor, not a new
  presentation framework.
- No change to Save/Apply routing, request reconciliation, bounded queues or the
  external-change banner. Those already exist from S1/S3 and are untouched.
- The joint setup/settings regression suite, the full-size PTY matrix and the
  revision-linked parent acceptance matrix (P2 and P3).

## Verified context

Observed facts:

- `crates/fluzo-core/src/settings.rs` is the single typed registry. The
  `group!` macro (line 210) builds each `SettingDescriptor` from a tuple of
  `(unit, constraint, privacy, application, description)`; every persistent
  setting and both collection templates (`ModelSettings`,
  `CapacityPoolSettings`) are declared through it. `Settings::entries()` adds one
  hand-written descriptor for `schema_version` (line 385) and post-pass overrides
  for maxima and the theme choice list.
- `crates/fluzo-core/src/configuration.rs:9` pins
  `CONFIGURATION_PROTOCOL: u32 = 5`. `ConfigurationValue` already carries the
  full `SettingDescriptor`, so a new descriptor field reaches the TUI without a
  new wire type.
- `crates/fluzo-tui/src/configuration.rs` has no basic/advanced concept at all:
  `keys()` (line 316) filters only by search text and collections staged for
  removal, so the editor currently shows every registered field at once.
- `Ctrl+D` (line 1023) restores only the focused key. The issue requires the
  action to affect "an explicit selection", so the checked set is currently
  ignored by the action it most obviously belongs to.
- `mutable()` (line 1864) already excludes `FixedFalse`, `SchemaVersion` and
  single-option choices, which is the set the restore action must also skip.
- `crates/fluzo-tui/src/terminal.rs` routes keys to `workspace.key(...)` before
  the setup `F4` branch, so in the ordinary configuration shell `F4` reaches
  `ConfigurationView` and is free. `F3` remains the model wizard.
- `crates/fluzo-runtime/src/configuration_tests.rs:72` asserts rejection of
  protocols `[1, 2, 3]`, so the existing test already encodes "older protocols
  are refused" and can be extended rather than rewritten.
- `crates/fluzo-tui/src/configuration_tests.rs:1011`
  (`selected_default_cancel_and_redacted_input_never_reuse_projection`) presses
  `Ctrl+D` with an empty checked set on the sensitive key `tools.shell.env` and
  requires exactly one staged edit. The focused-key path must keep restoring
  sensitive keys for this accepted behavior to hold.

Hypotheses, not verified:

- The exact basic/advanced split is a judgement call. It is reviewable as data in
  one file and does not affect correctness, only which fields are visible before
  `F4`.

## Decisions, dependencies, and risks

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | Tier lives on `SettingDescriptor`, assigned per field in the registry, not in the TUI. | The issue requires controls and validation to derive from the shared registry. A TUI-side allowlist would drift from the schema and could not cover collection members. |
| D2 | `F4` toggles advanced visibility in the configuration editor; the default is basic. | Reuses the accepted `F4` "advanced details" convention from setup instead of inventing a new control. `F4` is already free in this shell. |
| D3 | Tier filtering is uniform: it applies to the whole list, including palette-targeted views. | A field's tier must not depend on how the user arrived. Palette entries that land on an advanced-only namespace show an explicit reason line plus the `F4` route rather than a silent empty list. |
| D4 | Selection-scoped restore skips non-public fields; the focused single-key path does not. | A bulk action over off-screen, redacted rows cannot show the user what it clears. A focused key shows its default in the details pane, so a deliberate single reset is informed. This also preserves accepted S3 behavior. |
| D5 | Protocol bump to 6 rather than an optional field. | The tier is required for correct presentation; a silently-absent tier would make every field either always visible or always hidden depending on client. The runtime rejects the old protocol loudly instead. |

Dependencies:

- Accepted and integrated S1 (#44) save/apply service and S2 (#45) setup. Both are
  merged; this plan consumes their behavior and does not modify it.
- No dependency on UI-02 closure or C3 (#38).

Risks:

- Protocol bump touches every client of `CONFIGURATION_PROTOCOL`. Only this
  repository produces and consumes it, and the runtime rejection test is extended
  to cover protocol 5, so a stale client fails closed rather than mis-rendering.
- A wrong tier hides a setting a reviewer expects to see. Mitigation: the tier is
  one literal per field in one file, `F4` reveals everything, and the split is
  called out for human review in this plan.
- Bulk restore could clear a value the user did not mean to reset. Mitigation:
  draft-only, no dispatch, selection preserved, skipped keys counted in the status
  line, and `Ctrl+X` twice still discards the whole draft.

## Phases

### P1: Registry-owned tiers with selection-scoped default restore

- Outcome: opening configuration shows basic fields only; `F4` reveals advanced
  fields and back without losing filter, selection, checked keys or drafts;
  selecting several keys and pressing `Ctrl+D` stages the registry default for
  exactly those keys, reports what was restored and what was skipped, and issues
  no request.
- Owning repository: `fluzo-labs/fluzo`, branch `feat/typed-config-views`.
- Dependencies: none beyond accepted S1 (#44) and S2 (#45).
- Affected contracts:
  - `fluzo-core::settings`: new `SettingTier` enum; new `tier` field on
    `SettingDescriptor`; `group!` tuple gains a tier position; `schema_version`
    descriptor gains `tier`.
  - `fluzo-core::configuration`: `CONFIGURATION_PROTOCOL` 5 to 6.
  - `fluzo-runtime`: accepts protocol 6 only; protocol 5 now rejected.
  - `fluzo-tui::configuration`: `advanced` view state, tier-aware `keys()`,
    `F4` handler, selection-scoped `Ctrl+D`, updated footer and help text, and a
    reason line when the visible list is empty but hidden matches exist.
- Acceptance criteria:
  1. Every persistent setting and both collection templates carry a tier, and the
     existing parity test proves a live collection member's tier equals its
     template's tier.
  2. Runtime rejects protocols 1 through 5 and accepts 6.
  3. With advanced hidden, no advanced-tier key appears in the visible list; with
     `F4` pressed, it does. Toggling preserves filter, selection when still
     visible, checked set, staged edits and any pending request.
  4. When the filter matches only advanced keys and advanced is hidden, the list
     region states that no basic field matches and names `F4`; the header height
     math is unchanged.
  5. `Ctrl+D` with a checked selection stages the default for each selected
     mutable key that differs from its default; already-default keys, fixed keys,
     keys under a collection staged for removal, and non-public keys are skipped
     and counted; the checked set is unchanged; no request reaches the port.
  6. `Ctrl+D` with nothing checked keeps the accepted focused-key behavior,
     including for a sensitive key.
  7. A default restore never stages a collection removal and never consumes a
     request record.
  8. Existing configuration tests pass unchanged once the test helper opts into
     the advanced view.
- Actions:
  - [x] Add `SettingTier`, the `tier` field, the `group!` tier position, the
        `schema_version` tier and the tier parity assertion in core.
  - [x] Bump `CONFIGURATION_PROTOCOL` to 6 and extend the runtime protocol
        rejection test to `[1, 2, 3, 4, 5]`.
  - [x] Add `advanced` state, tier-aware filtering, the `F4` toggle, the
        selection-scoped restore and the updated footer/help/reason text in the
        TUI.
  - [x] Add TUI tests for hidden/revealed tiers, toggle preservation, empty-list
        reason, selection-scoped restore with skips, and focused-key regression.
  - [x] Update `CONFIGURATION.md`, `TUI.md`, `APPLICATION.md` and `AGENTS.md`
        for protocol 6, tier metadata and the selection-scoped restore.
  - [x] Update the configuration PTY case so pool navigation passes through the
        `F4` route, and assert the hidden-tier reason line on the way.
  - [x] Run applicable checks and record actual results.
  - [ ] Present evidence and stop for review before another phase.
- Verification: from the repository root with the pinned toolchain and a
  pre-populated cargo cache, `cargo fmt --all -- --check`;
  `cargo test -p fluzo-core -p fluzo-runtime -p fluzo-tui --locked --offline`;
  `cargo test --workspace --locked --offline`;
  `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`;
  `python3 -B -m unittest discover -s scripts -p 'test_configuration.py'`;
  `python3 scripts/check_dev_setup.py`; `python3 -m unittest discover -s scripts
  -p 'test_*.py'`.
  Expected evidence: zero failures in each, and the new tier/restore tests present
  in the fluzo-tui count.
- Risks and recovery: no migration is required because no persisted file format
  changes; the tier is derived, never stored. If the tier split is rejected in
  review, it is corrected by changing literals in `settings.rs` alone. If the
  protocol bump is judged too broad, reverting the single constant plus the two
  test lists restores protocol 5 behavior.

### P2: Joint setup/settings integration and full-surface verification evidence

- Outcome: the required new evidence from R2 "Verification and final acceptance"
  exists as automated tests: ordinary startup to palette/settings with devmenu
  disabled, explicit transition from setup, ordinary-host replacement refusal
  separated from controlled-host save/reload with independent file verification,
  registry coverage for every supported type and collection, redacted-value
  preservation, payload-limit rejection without partial effects, conflicts,
  failed and uncertain writes, late completion after closing a view, request
  record exhaustion with no automatic retry, and the buffer/PTY matrix at the
  minimum size plus 80x24, 120x40 and 160x50.
- Owning repository: `fluzo-labs/fluzo`.
- Dependencies: P1 accepted.
- Affected contracts: test-only; no production contract change expected.
- Acceptance criteria: each R2 bullet has a named test or a recorded reason it
  cannot be automated; live inference remains `not_run`; scoped human review of
  the new configuration surfaces is recorded.
- Actions:
  - [ ] Assemble the evidence list from R2 and map each item to a test or a gap.
  - [ ] Fill the gaps with deterministic buffer tests and focused PTY cases.
  - [ ] Record the matrix with revisions and failed attempts.
  - [ ] Stop for review.
- Verification: the suites documented in `AGENTS.md` plus the new PTY cases.
- Risks and recovery: PTY coverage is the slowest part and can be flaky; keep
  bounded real-time deadlines and isolate temporary roots.

### P3: Revision-linked parent acceptance matrix and documentation completion

- Outcome: every original #8 criterion maps to accepted S1/S2 evidence plus S3
  integration regressions, with remaining production integration requirements
  recorded as open rather than claimed by fixtures, and the documentation set
  consistent with the delivered behavior.
- Owning repository: `fluzo-labs/fluzo`, with `fluzo-docs` consulted for any
  criterion whose wording needs clarification.
- Dependencies: P2 accepted.
- Affected contracts: documentation only.
- Acceptance criteria: the matrix is revision-linked and reviewable; no criterion
  is marked complete on fixture evidence alone; the closure semantics for #46 and
  #8 remain explicitly unapproved.
- Actions:
  - [ ] Build the matrix.
  - [ ] Reconcile `TUI.md`, `CONFIGURATION.md`, `APPLICATION.md` and
        `AGENTS.md` with the final behavior.
  - [ ] Stop for review; do not close the parent.
- Verification: `python3 -m unittest discover -s scripts -p 'test_*.py'` plus
  the documentation review in `AGENTS.md`.
- Risks and recovery: none technical; the risk is overstating acceptance, which the
  matrix wording controls.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | 267 Rust tests, 73 Python tests, Clippy and fmt clean |
| P2 | implemented | passed | pending | 269 Rust + 74 Python tests green; R2 matrix recorded below; one new min-size PTY case |
| P3 | pending | not_run | pending | none yet |

Implementation, successful verification and human acceptance are separate
states. A selected commit message is not acceptance. `has_completed_all_phases`
stays `false` until every phase is implemented, all required verification passes
and all reviews are accepted.

## Next step

Review P1. The change is small, self-contained and reviewable as data (the tier
literals in `settings.rs`) plus one interaction change (`F4` and `Ctrl+D` in the
configuration editor). Reply "execute P2" only after P1 is accepted; otherwise
say "stop". P2 is a large evidence-gathering phase and should not start while the
tier split is still unreviewed, because a rejected split would invalidate the
coverage tests written against it.

## Evidence and tracking

P1 verification, run 2026-10-14 on the pinned toolchain with `--locked --offline`:

- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace --locked --offline`: 267 passed, 0 failed
  (cli 2 + bootstrap 4 + configuration 8, core 16, runtime 78 + http_simulator 12,
  tui 147; doc-tests 0).
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean.
- `python3 scripts/check_dev_setup.py`: passed (foundation graph, toolchain, LSP
  config and 12 skills).
- `python3 -B -m unittest discover -s scripts -p 'test_configuration.py'`: 6
  passed, 0 failed.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 73 passed, 0 failed;
  HTTP simulator passed, live inference `not_run`.

Open for P1 review: the basic/advanced split itself, and the fact that the palette
categories Harness, Pools, Storage and Telemetry now land on an empty list until
`F4`, because those namespaces are entirely advanced-tier.

### P2: R2 evidence matrix

Baseline: `origin/main` at `e7e63a7` (includes merged P1 #65). Branch
`feat/ui03-p2-evidence`. Each required R2 "Verification and final acceptance"
item maps to named tests in this repository at this revision, or to a recorded
non-automation reason. Test names are exact; run them with the focused suites
documented in `AGENTS.md`.

| R2 evidence item | Named tests |
| --- | --- |
| Ordinary startup to palette/settings with devmenu disabled; valid config without provider access | PTY `scripts/test_configuration.py::test_normal_settings_apply_cli_locks_resize_and_preserve_files` (launch `--no-dev-menu`, Ctrl+P palette into Configuration; fixture listener never accepted); TUI `workspace_has_no_synthetic_tasks_and_preserves_composer_across_settings`; PTY `scripts/test_setup.py::test_valid_configuration_skips_setup` |
| Explicit transition from setup; unchanged isolated demo and non-interactive contracts | PTY `test_setup_to_settings_keeps_create_only_and_request_identity`; PTY `test_normal_shell_honors_terminal_color_capabilities` (setup-handoff subTest); CLI `crates/fluzo-cli/tests/bootstrap.rs::demo_is_explicit_isolated_and_never_reads_project_configuration` and `execution_and_unknown_arguments_never_report_success`; PTY `scripts/test_setup.py::test_headless_discovery_and_explicit_selection_preserve_files` |
| Ordinary-host replacement refusal, including after creation | CLI `crates/fluzo-cli/tests/configuration.rs::create_only_policy_refuses_replacement_but_applies_presentation` (refusal projected before Save, file byte-identical); runtime `setup_creation_requires_current_confirmation_and_never_replaces`; runtime `cooperating_processes_cannot_write_while_directory_is_locked` |
| Controlled-host UI Save/reload with independent file verification | PTY `test_outside_change_banner_offers_keep_ours_then_reload` (harness re-reads `.fluzo` after Save and Reload); CLI `normal_view_real_save_apply_preserves_redacted_values_and_comments`; runtime `save_after_an_outside_edit_keeps_unrelated_outside_keys` and `reload_adopts_the_outside_version_and_clears_the_notice` |
| Registry coverage for all supported types and collections | TUI `registry_coverage_and_typed_inputs_include_collections_optional_and_escaped_maps`; CLI `collection_batch_and_all_value_types_round_trip_without_partial_save`; core `collection_metadata_is_tied_to_typed_fields` |
| Multi-field validation | runtime `collection_edits_validate_together_and_selected_save_reloads`; runtime `rejected_batch_is_atomic_and_projections_are_sanitized_bounded` |
| Defaults | core `canonical_defaults_and_metadata_agree`; TUI `restore_defaults_targets_the_explicit_selection_and_stays_in_the_draft`, `bulk_restore_skips_fixed_keys_without_staging_their_edits` |
| Selected-key preservation | runtime `read_only_and_explicit_selection_are_enforced`; TUI `restore_defaults_never_removes_a_collection_and_keeps_the_focused_key_path` |
| Redacted-value preservation | TUI `selected_default_cancel_and_redacted_input_never_reuse_projection`; CLI `normal_view_real_save_apply_preserves_redacted_values_and_comments` |
| Payload-limit rejection without partial effects | TUI `oversized_batch_and_paste_never_split_or_submit`; runtime `rejected_batch_is_atomic_and_projections_are_sanitized_bounded` |
| Actual presentation Apply; CLI precedence; saved/effective distinctions | PTY `test_normal_settings_apply_cli_locks_resize_and_preserve_files` (Applied/CliLocked/saved-vs-effective markers); runtime `selected_save_apply_cancel_and_cli_are_independent` (SavedFuture/ActiveSnapshot origins); CLI `review_regression_operational_save_does_not_poison_presentation_apply` |
| Cancel after Save/Apply | runtime `selected_save_apply_cancel_and_cli_are_independent` (Cancel after Saved+Applied leaves the file untouched); TUI `exit_save_waits_for_completion_and_retains_draft_on_rejection` |
| Unsupported operational changes; truthful restart-pending without selectable restart options | runtime `operational_apply_is_unavailable_and_restart_is_only_pending`; TUI `restart_pending_line_lists_exactly_the_snapshot_keys`, `restart_pending_empty_state_is_explicit`, `restart_pending_line_survives_an_unrelated_presentation_apply`, `restart_pending_line_stays_readable_across_sizes_and_fallbacks`, `restart_pending_keys_are_sanitized_like_other_configuration_text` |
| Conflicts | TUI `read_only_capacity_conflict_and_unknown_keep_drafts`; CLI `conflict_and_operational_apply_do_not_change_effective_or_replay`; runtime `external_create_edit_remove_and_stale_versions_never_clobber` |
| Failed and uncertain writes | runtime `file_failures_preserve_or_reconcile_without_claiming_rollback`; CLI `editor_reconciles_real_write_failure_without_partial_file_changes` |
| Late completion after closing a view | TUI `typing_is_local_and_late_validation_after_close_never_dispatches_save`, `reopen_before_late_validation_does_not_restore_save_intent`, `failures_and_closed_view_completion_are_not_replayed`, `f4_toggle_preserves_a_pending_request_and_its_identity` |
| Request-record exhaustion; no automatic retry or service replacement | CLI `request_exhaustion_is_projected_without_evicting_results`; runtime `worker_port_bounds_replay_and_quiesce_preserve_completion`, `lost_acknowledgement_and_worker_disconnect_never_replay_effects`, `queue_saturation_is_nonblocking_and_acknowledgement_is_not_completion` |
| Minimum-size fallback, 80x24, 120x40, 160x50 buffers; keyboard, hostile content, fallback rendering | TUI `layouts_keyboard_and_hostile_text_remain_bounded` (40x10 refusal through 160x50, ASCII, injected escape content); TUI `every_configuration_surface_uses_shared_dialogs_and_preserves_background` (60x16/80x24/120x40/160x50 x color/ASCII modes) |
| Focused PTYs: keyboard navigation, paste, resize, preserved input/focus, terminal restoration | PTY `test_normal_settings_apply_cli_locks_resize_and_preserve_files` (160x50/120x40/80x24 resize loop, bracketed paste, termios restoration, no clipboard/notify sequences); PTY `test_settings_at_minimum_size_keyboard_only_restores_terminal` (new: 60x16 minimum, keyboard-only palette→settings→list navigation→boolean stage→exit with zero writes, NO_COLOR fallback, termios restoration, provider listener never accepted); PTY `scripts/test_setup.py::test_setup_resize_preserves_draft`, `test_setup_signal_restores_terminal` |

Recorded non-automation reasons:

- Scoped human review of the new configuration surfaces: human gate by R2
definition; not automatable. Recorded as pending, not claimed by buffers or PTYs.
- "Verify forbidden effects with appropriately scoped instrumentation": the
fixture-listener-never-accept plus isolated HOME/XDG/temp root and byte-exact
file re-reads prove absence toward the fixture and the workspace only; absence
of all network traffic is not provable from one silent listener, per
`HTTP_SIMULATOR.md`.
- Live inference: `not_run` (not authorized for this phase).

Failed attempts during P2 (new 60x16 PTY case, all in the test itself, no
production change):

1. Expected `> agent.model` after Ctrl+U: wrong, `select_first` retains the
still-visible selection, so the selection stayed on `tui.theme`.
2. Expected `> max visible`: wrong, only Boolean rows render the short label;
non-boolean rows render the full key.
3. Expected the full `> tui.notifications.max_visible` row: wrong, the list
column truncates long keys at 60 columns; the test now matches the rendered
prefix.

P2 verification, run 2026-10-14 on the pinned toolchain with `--locked --offline`:

- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace --locked --offline`: 269 passed, 0 failed (cli 2 +
bootstrap 4 + configuration 8, core 16, runtime 78 + http_simulator 12, tui
149; doc-tests 0).
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean.
- `python3 scripts/check_dev_setup.py`: passed.
- `python3 -B -m unittest discover -s scripts -p 'test_configuration.py'`: 7
passed, 0 failed (new minimum-size case included; repeated 3x stable).
- `python3 -m unittest discover -s scripts -p 'test_*.py'`: 74 passed, 0 failed;
live inference `not_run`.

- Issue: [fluzo-labs/fluzo#46](https://github.com/fluzo-labs/fluzo/issues/46)
  (open, parent #8).
- Pull request: [fluzo-labs/fluzo#65](https://github.com/fluzo-labs/fluzo/pull/65),
  opened from `feat/typed-config-views` with `Ref: #46` so merging does not close
  the issue.
  GitHub issues remain the authority for status; this document is a design
  snapshot, not a parallel board.
- Untouched by this plan: the endpoint-consent plan, PR #62 and PR #63.
