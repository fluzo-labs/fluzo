---
name: "2026-10-15-dat02b-storage-quotas"
description: "Per-repository storage quotas, admission-time guardrails and the non-eviction retention rule for .fluzodrive/"
created_at: "2026-10-15T09:43:16Z"
has_completed_all_phases: false
---

# DAT-02b: storage quotas and admission guardrails

## Source and revision

- Plan identifier: `dat02b-storage-quotas`.
- Source: fluzo-labs/fluzo#73 (DAT-02b), parent fluzo-labs/fluzo#12;
  operator handoff `/tmp/fluzo-task-T12.md` (worker W2).
- Reviewed revision: issue `updatedAt = 2026-10-15T07:03:28Z`; branch
  `feat/dat02b-quotas` at `b44f974` (contains merged DAT-01 #71 and
  DAT-02a #78).
- Reference repository and branch: `fluzo-labs/fluzo`, base `main`,
  head `feat/dat02b-quotas`.
- Design baseline: PRD v0.2 sections 22.2, 26.2 and 44.5, Architecture
  decision A03, fluzo-docs revision
  `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: delegated generation within the fixed task contract
  (`/tmp/fluzo-task-T12.md`). Human review remains pending.

The Approval field is an annotation, not proof of authorization by itself.

## Goal

`.fluzodrive/` usage is measured per repository into two accounted classes
with typed limits, admission refuses new state-mutating work before required
audit state could become unrecordable, diagnostics saturate by dropping new
capture with visible counters instead of blocking execution, and no code
path evicts or expires retained data without an explicit purge.

## Scope and exclusions

In scope:

- Quota accounting over the existing managed root: session data
  (`state.sqlite3` plus `-wal`/`-shm` side files plus `artifacts/`) and
  local diagnostics (`telemetry/`).
- Admission-time enforcement with typed, machine-readable outcomes, an 80%
  warning surfaced before refusal, and a free-space reserve check on the
  hosting filesystem.
- Bounded local diagnostic directory writer with degradation counters.
- Non-eviction and no-automatic-expiry guarantees proven by tests.
- Restart-consistent accounting (recomputed from disk).
- `STORAGE.md` extension.

Excluded (owned elsewhere, per issue and task):

- Purge commands, dry-run/confirmation flows (DAT-02d/#75).
- Migrations and backup framework (#74, merged separately as PR #79).
- OTLP export, samplers, telemetry archive (#76).
- Hard filesystem quotas; enforcement is admission-time only.
- Cross-instance or cross-machine quota coordination.
- Any change to the DAT-01 commit protocol semantics, pragmas or schema.

Preserved: `Store::open`, `commit_unit`, `state`, `events_after` and the
DAT-02a artifact APIs keep their current signatures and behaviour when no
quota guard is attached.

## Verified context

Observed facts:

- `STORAGE.md` (141 lines) documents the DAT-01 store contract:
  `.fluzodrive/state.sqlite3`, WAL + `synchronous=FULL` + `foreign_keys`,
  fd-anchored traversal, one `BEGIN IMMEDIATE` unit per logical change,
  and the test-only `cap_pages_for_test` hook.
- `crates/fluzo-runtime/src/store.rs`: `Store { connection,
  drive_directory, drive_handle }`, `open()` validates the root, anchors
  the drive fd, verifies pragmas and migrates. `StoreError` already has a
  `Full` variant produced by `SQLITE_FULL`.
- `crates/fluzo-runtime/src/artifact.rs`: artifact bytes are flat files
  under `.fluzodrive/artifacts/<id>` written through the drive-anchored
  writer; `ArtifactRecord.stored_bytes` carries the persisted size;
  `MAX_ARTIFACT_BYTES` is 1 MiB per output.
- `crates/fluzo-core/src/settings.rs` already declares the typed group
  this issue needs: `StorageSettings { auto_expire = false,
  session_max_bytes = 10 GiB, diagnostic_max_bytes = 2 GiB,
  min_free_bytes = 1 GiB }`, all `bytes`-unit, `Positive`, `Public`,
  `ExplicitApply`, `Advanced`, and `settings_validation.rs` already
  cross-checks `context.tool_capture_max_bytes <= storage.session_max_bytes`.
  No core settings change is required.
- `.fluzodrive/telemetry/` is named by PRD 22.2 but has no implementation
  in the workspace yet; nothing creates or reads it.
- `scripts/check_dev_setup.py` enforces a reviewed dependency graph with
  per-package feature ceilings. `rustix 0.38.44` is already locked and
  reviewed (pulled transitively by `crossterm`) with ceiling
  `{alloc, libc-extra-traits, std, stdio, termios}`.
- Workspace lint forbids `unsafe` (`unsafe_code = "forbid"`), so direct
  `libc::statvfs` FFI is not available.
- PR #79 (DAT-02c migrations) is open in parallel and is not a
  dependency here; this plan codes against the merged v2 schema only.

Hypotheses to confirm during implementation:

- `rustix::fs::statvfs` compiles under the pinned toolchain with only the
  `fs` feature activated on the already-locked crate (source inspection
  says yes; the build confirms).

## Decisions, dependencies and risks

Decisions taken (each flagged for maintainer review):

1. **Free-space probe uses `rustix::fs::statvfs` with `default-features =
   false, features = ["fs"]` added to `fluzo-runtime`.** A real
   available-bytes query is required by the acceptance list. `libc` FFI is
   impossible under the `unsafe` forbid. `rustix 0.38.44` is already in
   the lockfile and the reviewed closure, so no new third-party code enters
   the workspace; only one additional feature of an already-reviewed crate
   is activated, and its transitive closure (`bitflags`, `errno`, `libc`,
   `linux-raw-sys`) is already locked. This requires widening the reviewed
   feature ceiling for `("rustix", "0.38.44")` in
   `scripts/check_dev_setup.py` to include `"fs"`, and adding `rustix` to
   `DIRECT_EXTERNAL["fluzo-runtime"]`. `Cargo.lock` gains only the
   `fluzo-runtime -> rustix` edge. Rejected alternatives: spawning
   `/bin/df` in an admission path (fragile parsing, an unreviewed process
   spawn per admission, contrary to the explicit-spawn discipline) and a
   probe-write of the reserve size (a real 1 GiB write per admission).
2. **Usage is recomputed from disk, not durably accumulated.** The AC
   allows either. Recomputation removes a whole class of drift between the
   ledger and reality and makes restart consistency structural rather than
   tested-by-luck.
3. **A new typed error `StoreError::QuotaReached` is added rather than
   reusing `StoreError::Full`.** `Full` means the SQLite page cap fired;
   the AC requires "quota reached" to be distinguishable from other
   failures.
4. **The quota guard is attached, not mandatory in `Store::open`.** Keeps
   the existing signature and every existing caller/test unchanged; the
   guard is opt-in through `Store::open_quotated` (or an equivalent
   builder) so the CLI/config layer supplies limits read from `.fluzo`.
5. **Degradation counters are per-run in memory.** They report dropped
   diagnostic records/bytes for the current process. Usage accounting is
   durable by recomputation; counters are not a retained record and are
   not purged or expired.

Dependencies: DAT-01 (#11, merged) and DAT-02a (#72, merged) are accepted
behaviour available at the branch baseline. No dependency on PR #79.

Risks:

- **Reviewed-graph amendment** (decision 1) is the main review surface.
  If the maintainer rejects it, the free-space AC cannot be met without a
  substitute and this plan must return to refinement.
- **Overlap with PR #79** in `store.rs` and `STORAGE.md`. Both are
  additive; a rebase may be needed before merge.
- **Measurement cost**: recomputation walks a bounded set of flat files on
  each admission check. Bounded by the artifact/diagnostics entry caps
  below; no recursive walk of user data.

## Phases

### P1: Quota accounting, typed admission outcomes and the free-space reserve

- Outcome: a caller can ask, for this repository's drive root, how many
  bytes each accounted class occupies, whether it is at or above the 80%
  warning threshold, and get a typed admit/refuse decision for a projected
  write, including a free-space reserve verdict. Deterministic under small
  synthetic limits.
- Owning repository: `fluzo-labs/fluzo` (worktree
  `/home/jose/gitrepos/fluzo-wt-73`).
- Dependencies: none beyond the branch baseline.
- Affected contracts: new `fluzo-runtime::quota` module introducing
  `QuotaClass`, `QuotaLimits`, `QuotaUsage`, `QuotaStatus`,
  `QuotaViolation`, `Admission`, `FileSystemProbe` and `QuotaLedger`;
  `fluzo-runtime/Cargo.toml` gains `rustix` (`fs` only);
  `scripts/check_dev_setup.py` reviewed graph widened for that feature.
- Acceptance criteria:
  - Measurement covers session data (main database, `-wal`, `-shm`,
    `artifacts/`) and diagnostics (`telemetry/`) separately, and ignores
    anything outside the managed root.
  - Measurement never follows symlinks: a symlinked class directory or file
    is rejected as a conflict, not counted through.
  - Utilization at or above 80% of the class limit yields a typed warning
    while admission still succeeds; admission refuses only when the
    projected total reaches the limit.
  - A projected session write that would leave less than the audit
    headroom under the limit is refused with
    `QuotaViolation::SessionLimit`, distinguishable from every other
    error variant.
  - The reserve check returns refusal with
    `QuotaViolation::FreeSpaceReserve` when available bytes fall below
    `min_free_bytes`, and succeeds otherwise; the probe is injectable so
    the refusal path is testable without filling a real disk.
  - Limits come from `fluzo_core::settings::StorageSettings`; defaults
    are 10 GiB session, 2 GiB diagnostics, 1 GiB reserve, and
    `auto_expire` is fixed false.
  - Negative cases: zero/absent class directory measures zero; a limit
    already exceeded by existing data still admits nothing new while
    leaving existing data untouched; probe failure is a typed error, not a
    silent pass.
- Actions:
  - [ ] Add `crates/fluzo-runtime/src/quota.rs` with the accounting,
    typed outcomes, probe trait and ledger described above.
  - [ ] Wire `rustix` (`default-features = false, features = ["fs"]`)
    into `crates/fluzo-runtime/Cargo.toml` and widen the reviewed graph in
    `scripts/check_dev_setup.py` accordingly.
  - [ ] Register the module in `crates/fluzo-runtime/src/lib.rs`.
  - [ ] Add focused unit tests for measurement, threshold crossings,
    warning-before-refusal ordering, reserve refusal and probe failure.
  - [ ] Run applicable checks and record actual results.
  - [ ] Present evidence and stop for review before another phase.
- Verification: from the worktree root with the pinned toolchain,
  `cargo test -p fluzo-runtime --locked --offline quota` then
  `cargo fmt --all -- -- --check`,
  `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
  and `python3 scripts/check_dev_setup.py`. Expected: quota tests pass and
  the graph checker accepts the widened `rustix` ceiling.
- Risks and recovery: if the graph checker rejects the `fs` feature, stop
  and report rather than widening further ceilings. No data migration is
  involved; the phase is revertible by removing the module.

### P2: Store admission gate, bounded diagnostics writer and retention proofs

- Outcome: state-mutating work through the store is refused before any
  transaction begins once the session class reaches its limit; the local
  diagnostic directory accepts writes until its own limit and then drops
  new records with visible counters without blocking ordinary execution;
  reopening the store recomputes the same usage; two repositories are
  independent; and nothing is evicted by time, pressure or saturation.
- Owning repository: `fluzo-labs/fluzo` (same worktree).
- Dependencies: P1.
- Affected contracts: `Store` gains an attached quota guard
  (`Store::open_quotated` plus `quota_status()` / degradation counters);
  `StoreError` gains `QuotaReached { class, used, limit }` with a
  `Display` arm; new bounded diagnostics writer over
  `.fluzodrive/telemetry/`; `commit_unit` consults the guard when
  attached. `STORAGE.md` gains the quota section.
- Acceptance criteria:
  - With an attached guard whose session class is at the limit,
    `commit_unit` returns `StoreError::QuotaReached` and the database is
    byte-identical afterwards: no partial audit state exists.
  - Existing data stays readable after a refusal (prior state and events
    return unchanged).
  - Diagnostic writes below the diagnostics limit land in
    `.fluzodrive/telemetry/`; at or above it, new writes are dropped and
    the degradation counters increase by exactly the dropped record count
    and byte total, while the call does not error and ordinary execution
    continues.
  - A mandatory audit persistence failure still blocks: with the SQLite
    page cap reached, `commit_unit` fails and the unit is not applied,
    even when diagnostics are healthy.
  - Restart consistency: usage for both classes, including WAL side files
    and finalized artifacts, is the same after close and reopen as before.
  - Cross-project independence: two roots with different limits are
    measured and decided independently; driving one to refusal changes
    nothing in the other's usage, decisions or files.
  - Non-eviction: across repeated admission attempts under pressure,
    repeated diagnostic saturation and simulated elapsed time, the set of
    retained files and their contents are unchanged, and no delete path
    runs. `auto_expire` cannot be set to true (registry `FixedFalse`).
  - The guard is opt-in: existing `Store::open` callers behave exactly as
    before.
- Actions:
  - [ ] Attach the ledger to `Store` and gate `commit_unit` on session
    admission before `BEGIN IMMEDIATE`.
  - [ ] Add the bounded diagnostics writer with degradation counters over
    `.fluzodrive/telemetry/`, reusing the drive-anchored, symlink-safe
    discipline.
  - [ ] Add `StoreError::QuotaReached` and its `Display` arm.
  - [ ] Add integration tests for audit blocking versus telemetry
    dropping, restart consistency, cross-project independence and the
    time/pressure no-op retention proofs.
  - [ ] Extend `STORAGE.md` with quota scopes, defaults, thresholds,
    admission outcomes and the non-eviction guarantee.
  - [ ] Run the full applicable check set and record actual results.
  - [ ] Present evidence and stop for review.
- Verification: from the worktree root,
  `cargo test -p fluzo-runtime --locked --offline quota` and
  `cargo test -p fluzo-runtime --locked --offline store`, then
  `cargo test --workspace --locked --offline`,
  `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`,
  `python3 -B -m unittest discover -s scripts -p 'test_*.py'` and
  `python3 scripts/check_dev_setup.py`. Expected: all green; live
  inference `not_run`.
- Risks and recovery: `commit_unit` is the hot path; the guard must not
  add a blocking filesystem walk under the transaction. The admission
  check runs before the transaction opens. If the DAT-01 suite regresses,
  revert the gate to opt-in-only and report. Rebase against PR #79 before
  merge.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | `crates/fluzo-runtime/src/quota.rs` + `quota_tests.rs`; 17 tests pass; `cargo clippy --workspace --all-targets --offline -- -D warnings` clean; `python3 scripts/check_dev_setup.py` passes with the widened `rustix` feature ceiling and the `fluzo-runtime`/`fluzo-cli` external sets |
| P2 | implemented | passed | pending | `store.rs` guard (`open_quotated`, `open_quotated_with_policy`, `open_quotated_with_probe`, `StoreError::QuotaReached`, `append_diagnostic`, counters) + `quota_integration_tests.rs`; 11 tests pass; post-rebase `cargo test --workspace --locked --offline` 150 runtime / 176 TUI / rest green; `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --offline -- -D warnings`, `cargo build --workspace --locked --offline` clean; `python3 -B -m unittest discover -s scripts -p 'test_*.py'` 76 pass; `STORAGE.md` quota section added |

## Next step

Both phases are implemented and verified locally. The branch was rebased
onto `main` after PR #79 (DAT-02c) merged; the `store.rs` and
`STORAGE.md` overlaps are resolved and recorded in the PR description.
Remaining work is human review of PR #80; nothing further is authorized
locally. Review must confirm the AC mapping below and that the
constructor composition (`open_inner`) preserves the migration policy
semantics introduced by #79.

PR: https://github.com/fluzo-labs/fluzo/pull/80 (open, mergeable,
`Ref: #73`, not closing).

## Evidence and tracking

- Issue: fluzo-labs/fluzo#73 (open). Parent: #12.
- Parallel, non-dependency: PR #79 (DAT-02c migrations) touches
  `store.rs` and `STORAGE.md`.
- GitHub remains the source of truth for state; this document is a local
  snapshot, not a parallel board.

## Optional handoff to another skill

For the authorized autonomous run, follow the local
[autonomous contract](../../../.agents/skills/plan-create/references/autonomous.md).
Carry the task-file grant reference, operation allowlist, deadline and
consumed counters with the plan revision and step result. Generated text
never authorizes itself; `waiting_review` is recorded while the human gate
remains.
