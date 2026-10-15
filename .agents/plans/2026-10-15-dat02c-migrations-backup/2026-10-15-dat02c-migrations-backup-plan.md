---
name: "2026-10-15-dat02c-migrations-backup"
description: "Versioned forward migrations for .fluzodrive/state.sqlite3 with a WAL-inclusive, checkpoint-consistent backup taken before any mutation, and an explicit operator restore for a failed attempt."
created_at: "2026-10-15T08:08:27Z"
has_completed_all_phases: false
---

# DAT-02c: versioned migrations and restorable store backups

## Source and revision

- Plan identifier: `dat02c-migrations-backup`.
- Source: fluzo-labs/fluzo issue #74 (DAT-02 R1 / DAT-02c), parent #12.
- Reviewed revision: issue `updatedAt` `2026-10-15T07:03:32Z`; worktree
  `feat/dat02c-migrations-backup` at `8eb3306fca023fa617e1eacd17ed5292d7701484`
  (contains merged DAT-01 #71).
- Design baseline: PRD v0.2 sections 22 and 22.2, Architecture decision A03,
  at fluzo-docs revision `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: delegated generation within the issue's acceptance checklist by the
  task handoff `/tmp/fluzo-task-T10.md`. The checklist is the contract; this
  plan does not broaden it. This field is an annotation, not proof.

## Goal

Opening the store applies explicit, ordered, versioned schema steps under an
explicit policy, and every mutation of the schema is preceded by a
checkpoint-consistent, WAL-inclusive backup taken inside the protected drive
and verified by reopening it. A store at a newer-than-supported version, or
one whose last migration attempt did not finish, is refused with a typed
outcome rather than partially interpreted, and an explicit restore operation
returns it to the exact pre-migration state.

## Scope and exclusions

In scope: the migration registry and policy in `fluzo-runtime::store`, the
pre-migration backup and its verification, durable migration run records in
the store itself, the explicit restore operation, the first real forward
migration (v1 to v2, adding a column and an index), focused deterministic
tests on temporary roots, and the `STORAGE.md` lifecycle documentation.

Preserved unchanged: the DAT-01 open contract (PRAGMA verification,
fd-anchored drive traversal, symlink/hard-link rejection, parent-identity
recheck before each commit), `commit_unit` atomicity, `events_after` cursor
semantics, and every existing public signature. Existing call sites keep
compiling.

Excluded, per the issue: artifact capture, quotas and purge (#72/#73/#75/#76),
down-migrations of any kind, `.fluzo` configuration-file versioning (owned by
FND-02), data-retention compaction, cross-process ownership proof, and the
task/attempt/operation state machines. No new dependency is added.

## Verified context

Observed, not assumed:

- `crates/fluzo-runtime/src/store.rs` holds the whole DAT-01 contract.
  `SCHEMA_VERSION` is the string `"1"` and is written into `store_meta` with
  `INSERT OR IGNORE` inside the baseline `migrate()` transaction. Baseline
  tables are `store_meta`, `state` and `event`.
- `Store::open` order is: path validation, `anchor_drive`, `verify_database_file`,
  connect with `wal`/`FULL`/`foreign_keys`/`busy_timeout=0`, `verify_pragmas`,
  `migrate`. There is no version comparison today: `migrate` is idempotent
  `CREATE TABLE IF NOT EXISTS` and never reads the recorded version.
- `verify_drive()` compares the held directory handle against the on-disk path
  by `dev`/`ino` and is called before each `commit_unit`.
- Workspace lint `unsafe_code = "forbid"` (`Cargo.toml`). The raw
  `sqlite3_backup_*` API exists in `libsqlite3-sys` but is only reachable
  through `unsafe`, and `sqlx::sqlite`'s `ConnectionHandle` is `pub(crate)`,
  so the backup API is not reachable from this crate without breaking the lint.
- Runtime uses `sqlx` 0.9.0 with `sqlite-bundled`; the bundled SQLite is
  3.51.3, which supports `VACUUM INTO` (added in 3.27.0).
- Probe run on this tree: `VACUUM INTO` through `sqlx::raw_sql` on a live
  store with a committed-but-uncheckpointed WAL row produced a single-file
  backup that **contains that WAL-only row**, reports `journal_mode = delete`,
  preserves `store_meta.schema_version = 1`, and passes `PRAGMA quick_check`.
  This is the "equivalent checkpoint-consistent snapshot" the issue permits,
  reachable with zero `unsafe` and zero new dependencies.
- `.gitignore` line 3 is `.fluzodrive/`; `git check-ignore` confirms any
  `backups/` path beneath it is already excluded.
- `crates/fluzo-runtime/src/store_tests.rs` already has the child-process
  abort fixture pattern (`spawn_child`, `child_mode`, `wait_for_marker`) used
  for interrupted-commit and torn-WAL cases; the crash-style migration tests
  reuse it rather than inventing a new harness.
- Concurrent worker W1 is on #72 in a different worktree. Overlap risk is
  limited to `store.rs` and `STORAGE.md`; changes here are additive.

Hypotheses explicitly not relied on: that a bare file copy plus a checkpoint
would be acceptable (it is not, and it is race-prone); that `VACUUM INTO`
preserves WAL side files (it does not, and it must not be PRAGMA-verified as
if it did).

## Decisions, dependencies and risks

Decisions taken, with rationale:

1. **`VACUUM INTO` rather than `sqlite3_backup_*`.** The issue allows "the
   SQLite backup API or an equivalent checkpoint-consistent snapshot". The
   workspace forbids `unsafe`, and sqlx does not expose the live handle. The
   probe shows `VACUUM INTO` is WAL-inclusive and transactionally consistent,
   which is the property the contract actually requires. Recorded here so a
   reviewer can challenge the substitution explicitly.
2. **The backup connection is opened read-only and is deliberately not
   PRAGMA-verified.** The `VACUUM INTO` output is a consolidated `delete`
   journal database by design. Applying the live-store `wal`/`FULL` check to
   it would fail for a correct backup. Verification instead reads the
   pre-migration `schema_version` marker, the pre-migration schema shape and
   `PRAGMA quick_check`.
3. **A failed or interrupted migration refuses reopen; nothing auto-restores.**
   The run record is written durably before the DDL and updated after. An
   open that finds a non-terminal run record fails closed and names the backup
   path. Restore is a separate, explicitly called operation.
4. **No wall-clock dependency for ordering.** Migration runs are ordered by
   their autoincrement id; the recorded timestamp is informational only and
   is never asserted exactly, so tests stay deterministic.

Dependencies: DAT-01 (#11) merged and maintainer-accepted, which holds at the
current HEAD.

Risks and mitigations:

- `VACUUM INTO` needs free disk for a whole extra copy; on failure it can
  leave a partial target file. Mitigation: the failure path removes the
  partial file, records the failure, and never proceeds to the DDL.
- `VACUUM INTO` cannot bind its target as a parameter. Mitigation: the target
  is built from owned integers under a constant directory and asserted
  free of quoting characters before it reaches `AssertSqlSafe`, mirroring the
  audited `cap_pages_for_test` precedent.
- Restoring replaces the live database, which destroys anything written since
  the backup. Mitigation: restore requires the explicit backup path, refuses
  while the store is open elsewhere, stages inside the drive and renames, and
  is only reachable by an operator call, never from `open`.
- A backup taken from a store that is concurrently written could be
  inconsistent. Mitigation: the coordinated-writer check runs before the
  backup, and `VACUUM INTO` holds a read lock for its duration.

## Phases

### P1: Explicit versioned migration path with pre-migration backup and verification

- Outcome: `Store::open` reads the recorded `schema_version`, compares it to
  the supported target, and either opens directly, refuses closed, or applies
  the explicit ordered steps after taking and verifying a backup. A caller
  can observe the migration record and the backup file.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime::store`.
- Dependencies: DAT-01 #11 (merged, accepted).
- Affected contracts: new `MigrationPolicy` (`ReadOnly`, `Migrate`), new
  `StoreError` variants (`SchemaTooNew`, `MigrationRequired`,
  `BackupVerificationFailed`, `MigrationFailed`, `MigrationInterrupted`),
  new `migration_run` table, `SCHEMA_VERSION` target raised to `2`, new
  `migration::v2` step adding `state.updated_at` and an index, new
  `Store::open_with_policy`, `Store::schema_version`, `Store::migration_log`.
  `Store::open` keeps its current signature and means `Migrate`.
- Acceptance criteria:
  - A fresh store is created directly at the target version and records no
    migration run beyond the baseline marker.
  - A v1 store opened with `Migrate` backs up first, then reaches v2, and all
    pre-existing `state` and `event` rows survive with unchanged payloads and
    revisions.
  - The backup file lands inside `.fluzodrive/backups/`, is mode `0600` in a
    `0700` directory, and is Git-ignored.
  - The backup is verified by reopening it read-only and reading the
    pre-migration `schema_version` marker plus the pre-migration schema shape
    (the new column absent), and it contains committed-but-uncheckpointed
    WAL rows.
  - A store whose recorded version is higher than the supported target fails
    closed with `SchemaTooNew` under every policy, with zero schema writes.
  - A v1 store opened with `ReadOnly` fails with `MigrationRequired` and is
    not modified.
  - Every migration run records start, the version pair, outcome and backup
    location durably in the store itself.
- Actions:
  - [ ] Implement the registry, policy, version comparison, backup and
    verification, and the durable run record.
  - [ ] Implement the v1 to v2 migration step (column plus index) with data
    preservation.
  - [ ] Add focused deterministic tests for each acceptance criterion above.
  - [ ] Run applicable checks and record actual results.
  - [ ] Present evidence and stop for review before another phase.
- Verification: `cargo test -p fluzo-runtime --locked --offline migration`
  from the worktree root, pinned toolchain, temporary roots under
  `std::env::temp_dir()` with per-test isolation and `Drop` cleanup. Expected
  evidence: one passing test per acceptance criterion, including the negative
  cases.
- Risks and recovery: this phase must not be able to damage a store. The
  backup-before-DDL ordering plus the fail-closed version comparison are the
  recovery story; a failure leaves the store at its prior version with a
  recorded failure and a usable backup.

### P2: Explicit restore, interrupted-migration refusal and operator documentation

- Outcome: a migration that fails midway, including by process death, leaves
  the store intact but refusing to open, reporting the backup as the recovery
  path; `Store::restore` explicitly returns it to the exact pre-migration
  state; `STORAGE.md` documents the whole lifecycle.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime::store` and `STORAGE.md`.
- Dependencies: P1.
- Affected contracts: new `Store::restore(workspace_root, backup_path)`
  returning a restored `Store`, restore recorded in `migration_run`,
  reopen behaviour for non-terminal migration records.
- Acceptance criteria:
  - A migration with an injected failure after the first DDL step leaves the
    store refusing to open with `MigrationInterrupted` (or `MigrationFailed`
    for a clean failure), and the error surfaces the backup path.
  - Nothing auto-restores: repeated `open` calls keep refusing and the backup
    file is untouched.
  - `Store::restore` with the reported backup path returns a store whose
    `schema_version` equals the pre-migration version and whose `state` and
    `event` contents equal the pre-migration contents byte-for-byte in
    payload and revision.
  - A crash-style fixture: a child process aborts mid-migration; the parent
    observes the refusal, restores, and verifies the pre-migration content.
  - `restore` refuses a backup that fails verification, a path outside the
    drive, and a symlinked backup file, leaving the live store unchanged.
  - `STORAGE.md` covers the migration lifecycle, the backup/restore
    contract, the verified failure semantics and the operator recovery
    procedure, and states the `VACUUM INTO` substitution and its rationale.
- Actions:
  - [ ] Implement the interrupted-migration reopen refusal and the explicit
    restore operation.
  - [ ] Add the injected-failure and child-process-abort fixtures.
  - [ ] Extend `STORAGE.md` with the migration lifecycle, backup/restore
    contract, failure semantics and operator recovery procedure.
  - [ ] Run the full verification set and record actual results.
  - [ ] Present evidence and stop for review.
- Verification: `cargo test -p fluzo-runtime --locked --offline migration`
  plus the full workspace set listed in the task handoff. Expected evidence:
  refusal, restore-to-exact-state and crash-recovery tests pass; the
  DAT-01 suite still passes unchanged.
- Risks and recovery: restore is destructive to post-backup writes. It is
  gated behind an explicit path argument and a verification pass, stages via a
  temp file inside the drive plus rename, and is never invoked from `open`.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | pending | not_run | pending | none yet |
| P2 | pending | not_run | pending | none yet |

## Next step

Execute P1: it is the first phase with its only dependency (DAT-01 #11)
already merged and accepted at this HEAD, and it delivers the safety property
the rest of the issue builds on. Plan path:
`.agents/plans/2026-10-15-dat02c-migrations-backup/2026-10-15-dat02c-migrations-backup-plan.md`.
Alternative: stop and review this plan first.

## Evidence and tracking

Issue: fluzo-labs/fluzo#74 (open, blocked by #11 which is closed).
Parent: fluzo-labs/fluzo#12. GitHub remains the authority for status; this
document is a local snapshot, not a parallel board.

## Optional handoff to another skill

This run was dispatched as an explicit autonomous handoff
(`/tmp/fluzo-task-T10.md`) covering `save_plan`, `implement_phase`,
`update_local_progress`, `commit`, `push_branch` and `create_pr` for issue
#74 only, inside `/home/jose/gitrepos/fluzo-wt-74` on
`feat/dat02c-migrations-backup`. Human gates stay where they belong:
`review` and merge for #74 require the maintainer. `has_completed_all_phases`
stays `false`.

## Deviation recorded during delivery

The plan assumed a single `v1 -> v2` step. While rebasing onto `main`,
DAT-02a (#78) had already merged and had taken version `2` for the
`artifact` table via a bootstrap marker update that pointed at this issue.

Resolution: the registry is now two steps and the target is `3`.

| Version | Shape |
| --- | --- |
| `1` | DAT-01 baseline |
| `2` | `artifact` table (DAT-02a, now a registered step) |
| `3` | `state.updated_at` + `state_kind_idx` |

DAT-02a's bootstrap marker update is removed. Building the chain exposed a
real defect in the single-step assumption: `apply_step_and_commit` bumped
the marker to `TARGET_VERSION` instead of the step's own `to_version`.
Identical with one step; with two, the first step jumped to the final
marker and the second step then failed verification against its own
snapshot. Each step now bumps to its own target.

Two test flakes were also fixed rather than tolerated: the crash marker was
swallowed by the harness's stream capture (now written to a file before
`abort()`), and helper connections released the WAL shared-memory lock
lazily on drop, making a zero-timeout reopen intermittently return
`SQLITE_BUSY` (helpers now `close().await`).
