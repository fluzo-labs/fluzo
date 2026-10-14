---
name: "2026-10-15-c3-devmenu-shared-save"
description: "Developer menu preferences persist and apply only through the UI-03 shared typed save/apply service; this slice adds the route-level integration evidence and documentation that C3's acceptance checklist requires on top of the already-typed editor."
created_at: "2026-10-15T09:00:00Z"
has_completed_all_phases: false
---

# C3: developer preferences on the shared save/apply path

## Source and revision

- Plan identifier: `c3-devmenu-shared-save` (UI-02 R1 / C3).
- Source: [fluzo-labs/fluzo#38](https://github.com/fluzo-labs/fluzo/issues/38),
  parent #7. The issue acceptance checklist is the contract.
- Branch: `feat/c3-devmenu-shared-save` created from `main` at
  `e7e63a7f4e0744001d64501c4728a3599c1e5051` (contains merged S1/S2/S3 and
  P1 tier work). Parallel PR #66 is untouched; no shared files expected.
- Design baseline: PRD 26.2/29.2.2 and architecture A06/A07 under the
  approved D0 revision, consulted through the issue text and `AGENTS.md`.

## Current state at this baseline (verified, not assumed)

- The ordinary palette's Developer Menu > Presentation settings already opens
  the S3 typed `ConfigurationView` with filter `tui.`
  (`crates/fluzo-tui/src/workspace.rs`, test
  `mvp_developer_entry_opens_real_presentation_settings_without_demo_actions`).
  The TUI has no filesystem/TOML or runtime-implementation access on this
  route; Save/Apply go out as `ConfigurationAction`s over
  `fluzo_core::configuration::ConfigurationPort`.
- The single writer is `fluzo_runtime::configuration::ConfigurationService`
  (S1). It revalidates every write, saves atomically with comment
  preservation, enforces CLI locks, and distinguishes saved/draft/effective
  with `SettingOrigin` (`SavedFuture` vs `ActiveSnapshot` vs CLI).
- `Workspace::poll` propagates `snapshot.effective_ui` into the live shell
  `Preferences` (session-applied projection).
- `ConfigurationChange` carries only `version`, `request` and `outcome`:
  change diagnostics are redacted by construction; the versioned sanitized
  effective-UI projection is `ConfigurationSnapshot.effective_ui` +
  `version`.
- The legacy synthetic `Overlay::Developer` (Alt+E/N/T, Ctrl+R preview
  defaults) exists only in the isolated demo shell, which never reads or
  writes `.fluzo`; it is not the ordinary route and is out of C3 scope.

## Gap this slice closes

The checklist items are satisfied at the service and view levels but lack
route-level evidence through the actual Developer menu with a real service
and real files, plus documentation of the C3 contract.

## Scope (single phase P1)

1. Route-level integration tests in `crates/fluzo-cli/tests/configuration.rs`
   (extending the existing S3 harness, no new framework), keyboard-driven
   through `Workspace`:
   - Developer menu -> typed editor -> Ctrl+S Save: only the selected
     persistent `tui.*` keys change in the file; comments and unrelated keys
     are preserved; no synthetic/preview/scenario strings are ever written.
   - Saved-future vs active-effective: after Save, the file holds the new
     values while `effective`/`effective_ui` remain the pre-save state; a
     CLI-locked key stays locked (draft saves, Apply returns `CliLocked`).
   - Ctrl+A Apply through the shared worker changes the real effective
     theme and `Workspace.poll` propagates it to `preferences.applied()`;
     the CLI-locked fps does not change.
   - Cancel (Ctrl+X confirmed) after a completed Save discards the draft
     without undoing the saved file; closing the view likewise leaves the
     completed Save in place.
   - Diagnostics: `snapshot.changes` records the Save and Apply outcomes
     with request ids and versions and carries no values by construction.
2. TUI gating test in `crates/fluzo-tui/src/workspace.rs`: with
   `dev_menu` disabled the palette hides the Developer Menu entry and no
   configuration view opens from it.
3. Documentation: `TUI.md` (C3 route and state model), `CONFIGURATION.md`
   (developer-menu persistence goes through the shared service; session-only
   vs persistent fixture evidence links), `APPLICATION.md` (port actions
   used by the route).

Excluded: the demo synthetic developer overlay, OBS-01 transport,
operational apply, setup machinery, any second writer, provider calls, task
execution, live inference.

## Acceptance criteria mapping

- Checklist 1 (typed port, runtime revalidation, no direct FS/TOML):
  route tests drive only `Workspace`/`ConfigurationView` public input; the
  only writer in the test is `ConfigurationService`, which revalidates.
- Checklist 2 (only selected persistent prefs; no synthetic content):
  file assertions in the Save step.
- Checklist 3 (preview/session-applied/saved-future/active-effective):
  pre/post Save and Apply assertions plus `preferences.applied()` propagation.
- Checklist 4 (provenance, CLI locks, pending restart, no false success):
  `cli_locked`, `SavedFuture`/`ActiveSnapshot` origins, status text only
  after settled completion; restart-pending already covered by
  `restart_pending_*` TUI tests and `operational_apply_is_unavailable_and_restart_is_only_pending`.
- Checklist 5 (invalid input, conflicts, failed writes, uncertain ack):
  covered by existing service/view tests (`external_create_edit_remove_and_stale_versions_never_clobber`,
  `file_failures_preserve_or_reconcile_without_claiming_rollback`,
  `lost_acknowledgement_and_worker_disconnect_never_replay_effects`); this
  slice references them rather than duplicating, and adds the route-level
  cancel/close preservation.
- Checklist 6 (redacted diagnostics, versioned sanitized projection):
  changes/outcome assertions in the route test.
- Checklist 7 (docs): item 3 above.

## Verification commands

Pinned toolchain, `--locked --offline`, from the worktree root:

- `cargo fmt --all -- --check`
- `cargo test --workspace --locked --offline`
- `cargo clippy --workspace --all-targets --locked --offline -- -- -D warnings`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`

Live inference: `not_run`. No provider listener exists in these fixtures;
the route performs no network and no task execution.

## Progress

| Phase | Implemented | Verification | Review | Notes |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | fmt/clippy clean; 272 Rust tests (3 new: 2 CLI route + 1 TUI gating); 73 Python tests; graph checker unchanged; live inference not_run |
