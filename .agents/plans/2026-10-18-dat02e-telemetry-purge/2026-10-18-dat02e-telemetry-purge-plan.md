# DAT-02e: local telemetry archive purge with writer quiesce (#76) — plan

Status: proposed -> executing -> implemented. Nothing here is accepted;
acceptance is maintainer review on the PR.

## Contract

Issue: fluzo-labs/fluzo#76 (parent #12). Baseline: PRD 22.2,
Architecture A03 at fluzo-docs `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
Ground truth: `STORAGE.md`, `crates/fluzo-runtime/src/{store,quota,telemetry,telemetry_archive}.rs`.

Two local diagnostic stores live under `.fluzodrive/telemetry/` and are
distinct from history (the DAT-01 journal):

- **diagnostic archive** — the bounded JSONL segments written by
  `JsonlArchive` (`segment-*.jsonl`);
- **pending export buffers** — the per-record diagnostic files captured
  through `Store::append_diagnostic`, awaiting a remote backend that the
  deferred OTLP export (#88) has not yet connected.

Acceptance criteria (issue):
1. `--dry-run` previews, per store, the archive and pending buffers with
   byte counts and the exact `.fluzodrive/telemetry/` path; zero side
   effects (verified state unchanged).
2. Confirmation-gated deletion: `--yes` required even in YOLO; a caller
   that cannot confirm deletes nothing and returns a structured outcome.
3. Writer/exporter quiescing BEFORE any deletion; refuses with a typed
   outcome naming the active component; never deletes under a live writer.
4. Stale buffers do not resurrect: a drained producer cannot rewrite old
   records; a new producer creates clearly-new records only.
5. Idempotent retry: a second purge removes nothing and reports zero.
6. Crash safety: a partial purge leaves a consistent tree; no partial
   delete outside the anchored root.
7. Quota accounting: purge releases DAT-02b diagnostic-class bytes
   consistently (before/after reported).
8. CLI surface: `fluzo telemetry purge --local --dry-run | --yes`.

## Scope decisions (recorded before implementation)

- **Sync service, no DB open.** The purge needs only an anchored drive
  handle and the quota ledger; both build without opening the SQLite
  database. This keeps the dry-run free of any database-creation side
  effect and keeps the CLI free of a tokio runtime. The service is
  synchronous; the diagnostic hub is thread-based, so quiesce is sync.
- **Store anchoring reused, not duplicated.** `Store::anchor_drive` gained
  a `create: bool` (`anchor_drive_with`) and `anchor_class` was extracted
  into a free `anchor_class_in(drive_handle, drive_directory, class_dir,
  create)`. The purge anchors the diagnostic directory from its own
  read-only drive handle with `create = false`, so a dry-run never
  creates `.fluzodrive`, the telemetry dir or any DB file.
- **Classification by file name.** `segment-<digits>.jsonl` is the
  archive; every other managed regular file is a pending buffer. A
  symlink, directory or any non-regular entry fails the whole scan closed
  (`UnsafeRoot`) so a tampered layout can never cause a delete outside
  the root.
- **Remote/shared data is never claimed cleared.** The result names what
  was not cleared (remote backend copies, shared aggregates); the
  Display never overclaims remote erasure.
- **History purge (#75) overlap.** None required: this work touches only
  the telemetry directory and the store's anchoring helpers (additive).
  No shared CLI helper was needed. W2's history purge stays independent.

## Phases

- [x] P1 — Reuse store anchoring: `anchor_drive_with(create)` and free
  `anchor_class_in`; `Store::anchor_class` delegates. Callers updated.
- [x] P2 — `telemetry_purge.rs`: `PurgeConfirmation`, `PurgePreview`,
  `PurgeResult` (with Display stating not-cleared), `PurgeError`
  (ConfirmationRequired / ActiveWriter / UnsafeRoot / Io), `preview()`
  and `purge()`. Order: confirm -> scan+validate root -> quiesce hub ->
  delete through anchored handle -> measure accounting.
- [x] P3 — CLI `telemetry purge --local (--dry-run | --yes)` in
  `fluzo-cli/src/main.rs`; structured JSON errors; help text updated.
- [x] P4 — Deterministic tests (`telemetry_purge_tests.rs`): preview
  counts/bytes; dry-run purity + ConfirmationRequired; confirmed purge
  removes both stores; idempotent retry; live-writer refusal naming the
  component; quiesced writer allows purge; drained producer cannot
  resurrect; DB untouched; symlinked entry and symlinked root rejected;
  preview rejects unsafe root; accounting released.

## Verification (actual results)

- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace --locked --offline`: all green (runtime lib
  181 passed incl. 14 `telemetry_purge_tests`; tui 176; http_simulator
  12; others green).
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 76 OK.
- `python3 scripts/check_dev_setup.py`: foundation graph, toolchain, LSP
  config and 12 skills checked.
- CLI smoke on a temp repo: dry-run previews 1 segment/16 B + 1 buffer/14
  B; no-flag -> `confirmation_required`; both flags -> `invalid_arguments`;
  `--yes` purges and empties the dir; second `--yes` removes 0.
- Live inference: `not_run`.

## Notes / limitations

- The `db-untouched` test uses `close_for_test` (T16 discipline) because
  sqlx defers the real close to a background thread; a plain `drop` left
  the WAL checkpoint racing the fingerprint (observed once in the full
  run, fixed by the deterministic close).
- "Pending export buffers" are the on-disk `append_diagnostic` files;
  the in-memory hub queues are the writer that quiesce settles and are
  reported in the preview as additional pending bytes (read-only).
