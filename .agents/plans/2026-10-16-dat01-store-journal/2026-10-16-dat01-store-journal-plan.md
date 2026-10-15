---
plan: dat01-store-journal
issue: https://github.com/fluzo-labs/fluzo/issues/11
has_completed_all_phases: false
---

# DAT-01: repository store, writer and durable journal

## Baseline

- Branch `feat/dat01-store-journal` from `origin/main` at `fdc2780`.
- Design: PRD v0.2 sections 21-24 and Architecture decision A03 at
  `fluzo-docs@60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Dependencies FND-02 (#2) and FND-03 (#3) merged and accepted.

## Dependency decision (checked before any crate addition)

`rusqlite` is not present in the pinned offline cargo cache. The cache does
contain `libsqlite3-sys` 0.27.0/0.37.0 (with the bundled SQLite amalgamation
and `bundled_bindings`) and the full `sqlx` 0.7.4/0.9.0 family. Workspace
lint forbids `unsafe_code`, which rules out driving `libsqlite3-sys`
directly from this workspace. Chosen binding: `sqlx = 0.9.0` with
`default-features = false, features = ["runtime-tokio", "sqlite-bundled"]`
(compiles `--offline`; the bundled C build was verified before any feature
work). Tokio was already a production dependency of `fluzo-runtime`.
The full transitive closure (113 package names, 107 new reviewed entries)
was added to `scripts/check_dev_setup.py` as a `(name, version)`-keyed
`SQLX_PACKAGES` policy with feature ceilings equal to the activated set at
review time. Shared nodes widened: `either 1.18.0` (TUI + sqlx union) and
`syn 2.0.119` (TUI entry extended with `full`/`visit-mut`).

## Scope

`fluzo-runtime::store` only. No TUI coupling, no protocol DTOs (task
transport is a later story), no changes to the configuration service.

1. Protected `.fluzodrive/` root: fd-anchored traversal from `/` with
   `O_NOFOLLOW|O_DIRECTORY|O_CLOEXEC`, exclusive 0700 creation, symlink
   and non-directory rejection, database-file symlink/hard-link rejection,
   and a dev/ino parent-identity recheck before every commit (same
   discipline as the `.fluzo` configuration writer). `.fluzodrive/` is
   already git-ignored.
2. SQLite opened with `journal_mode=WAL`, `synchronous=FULL`,
   `foreign_keys=ON`, `busy_timeout=0`; each PRAGMA is read back at open
   time and the open fails closed (`PragmaNotEnforced`) if the enforced
   value differs.
3. One logical unit = one `BEGIN IMMEDIATE` transaction carrying state
   upserts plus journal inserts; either all rows land or none.
   Foreign-key violations discard the whole unit.
4. Crash fixtures expose unknown outcomes and recovery never half-applies:
   interrupted commit (child aborts mid-transaction), torn WAL tail
   (truncated WAL recovers only complete checksum-valid transactions),
   writer-lock loss (busy refusal, then recovery after the holder dies),
   and disk-full simulation via `PRAGMA max_page_count` producing the
   real `SQLITE_FULL` error path.

## Acceptance criteria mapping

| AC | Evidence |
| --- | --- |
| `.fluzodrive/` protected/ignored | `symlinked_drive_directory_is_refused`, `plain_file_named_drive_directory_is_refused`, `hardlinked_database_file_is_refused`, `drive_directory_is_created_private`, `drive_replacement_after_open_is_refused_at_commit`; `.gitignore` already lists `.fluzodrive/` |
| WAL/FULL/foreign keys verified | `open_enforces_wal_full_synchronous_and_foreign_keys` (PRAGMAs read back at open; mismatch fails closed) |
| State/events commit together | `commit_unit_persists_state_and_events_together`, `foreign_key_violation_discards_the_whole_unit` |
| Crash fixtures expose unknown outcomes | `interrupted_commit_rolls_back_completely`, `torn_wal_tail_recovers_last_complete_transaction`, `writer_lock_is_refused_and_recovers_after_holder_dies`, `disk_full_simulation_fails_closed` |
| Evidence + docs linked | This plan, `STORAGE.md`, and the PR description link commands and results |

## Verification (run 2026-10-16, pinned toolchain, `--locked --offline`)

- `cargo fmt --all -- --check`: clean.
- `cargo test --workspace --locked --offline`: 284 passed, 0 failed
  (12 new store tests in `fluzo-runtime`).
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 74 passed, 0 failed.
- `python3 scripts/check_dev_setup.py`: passed (widened graph policy reviewed above).
- Live inference: `not_run`.

## Verification limits

- Disk-full is simulated through SQLite's own `SQLITE_FULL` page-cap path,
  not a physically full filesystem; the error class and rollback behavior
  are identical at the SQLite boundary.
- The coordinated-writer contract covers this tool's own processes;
  arbitrary external editors of `.fluzodrive` and network filesystems
  remain outside the contract, as with the configuration writer.
- Cross-process ownership proof, migration backups and purge (DAT-02+ and
  the storage sections of PRD 24) are explicitly later child work, not
  claimed here.

## Progress

| Phase | Implemented | Verification | Review |
| --- | --- | --- | --- |
| P1 store module + protection | done | passed | pending |
| P2 crash fixtures | done | passed | pending |
| P3 docs + graph policy | done | passed | pending |

## Next step

Review. Nothing is accepted from this plan; acceptance is the maintainer's
gate on the PR. Do not close #11 from the implementation PR.

- Issue: [fluzo-labs/fluzo#11](https://github.com/fluzo-labs/fluzo/issues/11)
