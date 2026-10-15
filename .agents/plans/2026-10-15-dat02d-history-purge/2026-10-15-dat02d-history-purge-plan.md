---
name: "2026-10-15-dat02d-history-purge"
description: "History purge with dry-run, unbypassable confirmation and shared-reference protection over the DAT-02 store"
created_at: "2026-10-15T14:04:11Z"
has_completed_all_phases: false
---

# History purge with dry-run, confirmation and shared-reference protection

## Source and revision

- Plan identifier: DAT-02d / 2026-10-15-dat02d-history-purge.
- Source: <https://github.com/fluzo-labs/fluzo/issues/75> (read in full at
  execution start; parent <https://github.com/fluzo-labs/fluzo/issues/12>).
- Reviewed revision: issue body as fetched during this run; worker task file
  `/tmp/fluzo-task-T18.md`.
- Reference repository and branch: `fluzo-labs/fluzo`,
  `feat/dat02d-history-purge` from `origin/main` at
  `957dbb79004e1927726c48b8665f600988d970ff` (contains DAT-01 #71,
  DAT-02a #78, DAT-02b #80, DAT-02c #79, OBS-01 #89, CI hardening #81).
- Design baseline: PRD v0.2 sections 22.2 and 26.2, Architecture decision
  A03, at `60c5b0732fb710cdf705476cee8d9156a5ecd971` of `fluzo-docs`.
- Approval: pending. The task authorises implementation and a `Ref:` PR, not
  acceptance or merge.

## Goal

`fluzo history list` and `fluzo history purge` can preview and remove one
repository's session history from durable records only, with a dry-run that
provably changes nothing, a confirmation that YOLO cannot bypass, shared
artifacts preserved through their surviving owners, and idempotent retry
after a partial failure.

## Scope and exclusions

In scope: the store-level purge contract, the session-attachment primitive
that makes active-session protection durable, the CLI surface named in the
issue, quota-release consistency, and the `STORAGE.md` extension.

Out of scope: telemetry/local-diagnostic purge (DAT-02e #76), remote or
exported-record deletion, forensic-erasure claims, automatic cleanup
triggers, real session/task runtime, and any change to the accepted
DAT-02a finalization order or DAT-02b accounting rules. History deletion
must not imply diagnostic copies were removed.

## Verified context

Observed in the tree at the pinned base:

- `state(id, kind, payload, revision, updated_at)`, `event(seq, state_id
  REFERENCES state(id), kind, payload)`, `artifact(id, state_id
  REFERENCES state(id), kind, provenance, stored_bytes, content_hash,
  capture_flags)`, plus `audit` (OBS-01) and `migration_run`.
  `TARGET_VERSION` is 4 with a contiguous three-step registry.
- Artifact content files are named by artifact id under
  `.fluzodrive/artifacts/`, opened `O_NOFOLLOW`, written only by the
  drive-anchored writer in `artifact.rs`.
- Quota usage is **recomputed from the filesystem on every check**
  (`quota.rs`), so a purge releases usage structurally rather than by
  adjusting a counter.
- `Store::anchor_class` / `anchored_path` give fd-anchored, `O_NOFOLLOW`
  traversal of managed class directories; `verify_drive()` refuses a
  replaced drive.
- DAT-02a explicitly excludes cross-session deduplication, and its
  `STORAGE.md` section states the persisted record "supports
  shared-reference analysis for purge (DAT-02d) without scanning bytes".
- There is **no session or activity model** in the store today: `state`
  rows are the only durable unit, and nothing records attachment.
- The CLI is hand-rolled `OsString` dispatch in `main.rs` / `startup.rs`;
  there is no clap parser and no `history` command.

Resolved design questions (decisions, not observations):

- **Session** means a `state` row. Listing, sizing and purging are defined
  over that row, its `event` rows and its `artifact` rows.
- **Shared artifact** means an artifact whose non-empty `content_hash` is
  also owned by another session that is not being purged. This is the
  analysis DAT-02a's recorded SHA-256 exists to support. Because
  `artifact.state_id` is a single-owner foreign key and DAT-02a rules out
  deduplication, a shared artifact cannot be kept as a row of the purged
  session: deleting the session row would violate the foreign key. The
  protection is therefore that the artifact remains present and verifiable
  through the surviving session, and the purge **verifies that at deletion
  time** before removing the purged copy, so a concurrently vanished
  holder turns the item exclusive rather than destroying the last copy.
- **Active/attached** needs a durable record to survive across processes,
  so this plan adds one small table rather than passing a caller-supplied
  set that nothing would populate.

## Decisions, dependencies, and risks

- Depends on accepted DAT-02a reference model and DAT-02b accounting; both
  are merged on the base.
- Adds schema version 5 (`session_attachment`). Forward-only, registered,
  backed up and audited by the existing DAT-02c path. No down-migration;
  the existing backup/restore story covers recovery.
- Risk: deleting files after the SQL commit is a two-step durability
  boundary (same shape as DAT-02a's finalize/reference split). A crash
  between them leaves a removed row with a surviving file, which the retry
  path must collect. This is deliberate: holding the write lock across
  filesystem deletion would violate the "no transaction across I/O wait"
  rule.
- Risk: `--all` must never widen beyond the selected repository. Mitigated
  by deriving every path from the already-anchored drive of the opened
  store and never from a caller-supplied path.
- Live inference: `not_run`. No provider contact anywhere in this plan.

## Phases

### P1: Store-level purge contract with shared-reference and active-session protection

- Outcome: `Store::history_summary`, `Store::plan_purge` and
  `Store::purge_sessions` run against a temporary repository with fixture
  sessions and artifacts, producing a typed plan and a typed per-item
  outcome, with dry-run byte-identical state, shared/exclusive split,
  active-session refusal, partial-failure retry and quota release.
- Owning repository: `fluzo-labs/fluzo`, crate `fluzo-runtime`.
- Dependencies: merged DAT-02a (#72/#78), DAT-02b (#73/#80), DAT-02c
  (#74/#79), CI fixture hardening (#81).
- Affected contracts: new `fluzo-runtime::purge` module with `PurgePlan`,
  `PurgeOutcome`, `PurgeRefusal` and `Confirmation`; new
  `MigrationStep::AddSessionAttachment` (v4 -> v5) and `TARGET_VERSION` 5;
  `Store::attach_session` / `Store::detach_session`.
- Acceptance criteria:
  - listing reports ids, sizes and artifact counts from durable records
    only, and performs no deletion or external contact;
  - dry-run leaves the store byte-identical (database file and artifact
    tree hashed before and after);
  - exclusive artifacts are removed with their content files; shared
    artifacts remain present and verifiable under the surviving session;
  - reference state is read at deletion time, so a concurrently removed
    holder reclassifies the item instead of destroying the last copy;
  - an attached session is refused and the refusal names it;
  - deletion without confirmation deletes nothing and returns promptly;
  - an injected mid-purge failure leaves the remaining files collectable,
    and the retry completes without damaging survivors or double-releasing
    quota;
  - a symlinked artifact file is refused and never followed;
  - post-purge `SessionData` accounting equals a fresh filesystem scan.
- Actions:
  - [x] Add the attachment migration step and registry contiguity coverage.
  - [x] Implement the purge module and its deterministic tests.
  - [x] Run focused checks and record actual results.
  - [x] Stop for review before the CLI phase. (Delivered in one branch per
        the task brief; review gate is this PR.)
- Verification: `cargo test -p fluzo-runtime --locked --offline purge`,
  `cargo test -p fluzo-runtime --locked --offline migration`, plus the
  workspace checks.
- Result: 19 purge tests pass; runtime lib 186 pass; workspace green;
  clippy clean. Every acceptance criterion above has a named test.
- Risks and recovery: schema bump is covered by the existing pre-migration
  backup and `Store::restore`.

### P2: CLI surface with unbypassable confirmation

- Outcome: `fluzo history list`, `fluzo history purge --session <id>
  [--dry-run]` and `fluzo history purge --all --yes` work against a real
  temporary repository, print the named repository and `.fluzodrive/`
  path, and refuse non-interactive deletion without confirmation.
- Owning repository: `fluzo-labs/fluzo`, crate `fluzo-cli`.
- Dependencies: P1.
- Affected contracts: new `history` dispatch in the CLI, help text for the
  purge surface, structured exit codes for refusal classes.
- Acceptance criteria:
  - `--all` is scoped to the selected repository with a second repository
    present and untouched;
  - YOLO does not bypass confirmation: the confirmation requirement is
    independent of any YOLO flag and is asserted as such;
  - non-interactive without `--yes` returns a structured outcome and
    deletes nothing;
  - dry-run output names the repository, the drive path, the removal list
    and the preserved shared list;
  - unknown session, attached session and unsafe-path refusals each produce
    a distinct structured outcome.
- Actions:
  - [x] Implement the CLI dispatch and rendering.
  - [x] Add CLI-level tests through the store surface and PTY coverage for
        the interactive confirmation path.
  - [x] Run focused checks and record actual results.
- Verification: `python3 -B -m unittest discover -s scripts -p
  'test_history.py'` (13 tests, three of them through a PTY).
- Result: all 13 pass against the real binary. `cargo test -p fluzo-cli`
  has no `history` unit tests because the CLI holds no policy; every
  decision is delegated to the store contract and tested there.
- Note: `fluzo-cli` may not take a direct external dependency, so the
  async runtime lives in `fluzo_runtime::history`, a synchronous facade
  that builds a private current-thread executor per call.
  `check_dev_setup.py` passes unchanged, confirming no new production
  dependency.
- Risks and recovery: none beyond P1; the CLI performs no policy of its
  own and delegates every decision to the store contract.

### P3: Documentation and full verification

- Outcome: `STORAGE.md` documents the purge surface, confirmation
  semantics, shared-reference rules and verified failure behaviour, and
  the whole verification set passes on the final tree.
- Owning repository: `fluzo-labs/fluzo`.
- Dependencies: P1, P2.
- Affected contracts: `STORAGE.md` (including correcting the stale
  "purge previews ... are later child work" boundary line).
- Acceptance criteria: documentation matches implemented behaviour,
  states what is and is not cleared, and repeats the no-forensic-erasure
  limit; every verification command is recorded with its actual result.
- Actions:
  - [x] Extend `STORAGE.md`.
  - [x] Run the full verification set and record results.
  - [x] Open the PR with `Ref: #75` and stop for review.
- Verification: the command list in the task brief.
- Result: `cargo fmt --all -- --check` pass; `cargo clippy --workspace
  --all-targets --locked --offline -- -D warnings` pass; `cargo test
  --workspace --locked --offline` pass; `python3 -B -m unittest discover
  -s scripts -p 'test_*.py'` pass (89 tests, was 76); `python3
  scripts/check_dev_setup.py` pass.
- Risks and recovery: none.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | pass | pending | 19 purge tests; runtime lib 186; clippy clean |
| P2 | implemented | pass | pending | 13 CLI tests incl. 3 PTY; dev-setup unchanged |
| P3 | implemented | pass | pending | STORAGE.md section; full suite green |

## Honest limits

- The database file does not shrink on `DELETE`. SQLite reuses free pages
  and only `VACUUM` returns them, so the bytes a purge releases are the
  artifact files, not the journal. `VACUUM` is out of scope here and the
  quota test asserts the release accordingly.
- The telemetry / local-diagnostic archive is **not** purged. That is
  DAT-02e (#76). Deleting history does not imply diagnostic copies are
  gone, and `STORAGE.md` says so.
- `session_attachment` is written only by tests today: the runtime has no
  session-attach path yet because there is no task runtime. The contract,
  the durability across a reopen and the refusal are real and tested; the
  producer arrives with the execution milestone.
- No forensic-erasure or secure-delete claim. Files are unlinked, not
  overwritten.
- Live inference: `not_run`. No provider was contacted by anything in
  this delivery.
- Human review of this PR is still required; `has_completed_all_phases`
  stays `false` until that happens.

## P4: CI failure diagnosis and store-lifetime fix (implemented)

The first CI run of this PR failed consistently (twice, same test, same
error) on `scripts/test_history.py::
test_confirmed_purge_removes_exclusive_and_preserves_shared` with
`{"code":"storage_unavailable","message":"storage unavailable: Busy"}`.
Every local run passed, including under CPU load and pinned to 1 and 2
CPUs.

Diagnosis, in order of elimination:

- The error is `StoreError::Busy`, which the store raises only because it
  is deliberately configured with `busy_timeout = 0`. Nothing in the
  purge path waits.
- The failing call was the *second* purge in the test (gamma), not the
  first, so the store had already been opened and closed once by the CLI.
- A 200-iteration same-process probe (open, read, drop, reopen) showed
  zero Busy. A 300-iteration variant that took the write lock on the
  second open also showed zero. The trigger is therefore not observable
  in one process.
- The CLI opened the store once per step of a single command -- listing,
  plan, delete -- and dropped each connection instead of closing it.
  Dropping defers the real close to a background thread, so the next
  open re-entered the WAL index while the previous connection was still
  unwinding. The repo already documents this hazard at the store's
  test-only close helper; the facade simply never used it.

Fix:

- One `History` owns one runtime and one open store. A command opens
  once, runs every step against that connection, and closes it
  explicitly on every exit path including refusals.
- `Store::close_for_test` becomes production `Store::close`, since the
  capability is needed outside fixtures.
- `History::open` retries only the transient open-time `Busy` raised
  while a previous short-lived opener's lock is still being released --
  the same class the restore probe already accepts. No write path
  retries, and a store held by a live writer still refuses after the
  bounded attempts, so the one-writer-per-workspace contract is
  unchanged.
- Every store failure carries its stage (`open`, `list`, `plan`,
  `delete`, `reclaim`, `close`). The original failure could not be
  placed; those stages mean different things. Same self-diagnosing
  discipline as #81.
- Four facade regression tests added, because the failure depends on
  scheduling and must not be left to luck: repeated command cycles never
  meet a busy store; explicit close lets the next opener write
  immediately; store failures name their stage; drive absence is
  reported rather than created.

Rebased onto `main` (`d84e3dd`) after #90, #91 and #93 merged; the only
conflict was the CLI help text and subcommand dispatch, resolved by
keeping both the telemetry and history surfaces.

Verification after the fix: `cargo fmt --all -- --check`, `cargo clippy
--workspace --all-targets --locked --offline -- -D warnings`, `cargo
test --workspace --locked --offline` (207 runtime-lib, 179 tui, 31 core,
12 http_simulator, 10, 4, 2 -- all pass), `python3 -B -m unittest
discover -s scripts -p 'test_*.py'` (89 pass), `python3
scripts/check_dev_setup.py`. CI run 38766453797: `foundation` success,
including the previously failing case and the new facade cycle test.

## Next step

Human review of the pull request. If the re-ownership model for shared
artifacts (documented in `STORAGE.md` under "History purge (DAT-02d)")
is accepted as the intended reading of "preserved", nothing further is
blocked in this issue. DAT-02e (#76) remains the next eligible child of
#12.

## Evidence and tracking

- Issue: <https://github.com/fluzo-labs/fluzo/issues/75> (parent #12).
- Pull request: <https://github.com/fluzo-labs/fluzo/pull/92> (`Ref: #75`).
- CI: run 38766453797 (`Development foundation`) success on
  `a0316a7`.
- GitHub issues and the Fluzo Delivery project remain the state authority;
  this document is a design snapshot, not a parallel board.
