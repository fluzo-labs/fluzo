# T32: RUN-04 session recovery and private task-control channel — plan

Status: approved for execution by the T32 worker dispatch (grant below).
Ref: #18. PRD 17, 17.1, 17.2, 22.1, 24 and Architecture A02, A03 at
fluzo-docs revision 60c5b0732fb710cdf705476cee8d9156a5ecd971.

## Grant (from the T32 dispatch)

- `approval_ref`: T32 worker dispatch (W2), explicit "execute end to end".
- `repository` / `workspace`: `fluzo-labs/fluzo`,
  `/home/jose/gitrepos/fluzo-wt-run04`, branch `feat/run04-session-recovery`.
- `work_items`: #18 (RUN-04), phases P1-P3 below, baseline revision above.
- `allowed_paths`: `crates/fluzo-runtime/src/`, `crates/fluzo-core/src/`,
  `crates/fluzo-cli/src/`, `RECOVERY.md`, `.agents/plans/`, `AGENTS.md`.
- `operations`: `analyze`, `save_plan`, `implement_phase`,
  `update_local_progress`, `commit`, `push_branch`, `create_pr`, `continue`.
- `content_authority`: delegated generation within the contract above.
- `destinations`: PR base `main`, head `feat/run04-session-recovery`,
  non-closing `Ref: #18` (never `Closes`).
- `checks`: the pinned-toolchain command set in the Verification section.
- `human_gates`: phase review and PR merge remain human.
- `limits`: three phases, one PR, stop at the first unresolvable blocker.

## Contract

Three verbatim acceptance criteria drive everything:

1. **Open/replay is read-only.**
2. **Resume reconciles versions/unknown outcomes and preserves budget.**
3. **Inspection clients cannot start a second writer or restore YOLO.**

Key rules to encode, from PRD 22.1 / 24 / 17.2 and A02 / A03:

- Reopening a persisted session MUST NOT contact model or decision
  providers, execute tools, retry requests, restore files or dispatch
  queued actions. Selecting a session is not consent to continue.
- Replay is a read-only rendering of persisted events. Deterministic
  replay means consistent reconstruction from the same recorded events,
  not deterministic re-execution. Gaps and unknown outcomes stay visible.
- On recovery, attempts recorded as running with no durable terminal
  outcome are `Interrupted`; operations that may have started with no
  confirmed result are `outcome unknown`, never failed, successful or
  "safe to repeat". An operation id is not exactly-once proof.
- Before resuming, revalidate workspace identity, Git branch/revision and
  recorded file versions. A missing or mismatched workspace blocks
  continuation; the runtime MUST NOT silently substitute its current
  startup directory. Stale verification claims and edit approvals are
  invalidated and surfaced.
- Pending permission requests and unused operation-specific approvals
  from the interrupted attempt do NOT carry over. Deliberately
  configured standing grants remain subject to current policy and scope.
- Known budget consumption carries forward rather than resetting
  silently; uncertain usage is disclosed and any extension is explicit.
- Explicit resume creates a new attempt and trace linked to the original
  task/session and prior attempt. Reconciliation decisions are persisted.
  No automatic rollback and no automatic command replay.
- YOLO is never restored by recovery. It has no settings key and is
  process-memory only (RUN-02); a resumed session starts with it off.
- Control requests targeting work owned by another Fluzo process MUST
  use a private, owner-verified control channel or be refused, never
  signal arbitrary processes inferred from a task label. Force stop
  without explicit confirmation returns `confirmation_required` rather
  than waiting.
- Inspection clients do not own the runtime lifetime and cannot start a
  second writer (A02: one active runtime owner per workspace).

## Verified context

Observed in this tree at `0cc542c` + `d14105e` (origin/main):

- `store.rs` owns `.fluzodrive/state.sqlite3` with a generic
  `state(id, kind, payload, updated_at)` plus an `event` journal,
  `commit_unit` for atomic state+event commits, WAL + `synchronous=FULL`
  verified at open. A *session* at this layer is a `state` row with its
  `event` rows (see `purge.rs` header).
- `Store::open` is read-write and creates/migrates. There is **no**
  read-only open path; `verify_backup` in `migrations.rs` shows the
  `read_only(true)` connect pattern to follow.
- `workspace.rs` gives `WriterOwnership` (held `flock` descriptor, not a
  DB row), `OwnerRecord { pid, acquired_ms, workspace, owner,
  reconciled_from, released_ms }`, `verify_still_owned()` and
  `OwnerProbe { Free, Held, HeldUnknown, Released, Abandoned }`.
  `OwnerRecord.owner` is a per-acquisition nonce and is the natural
  owner-verification token for a control channel; a pid alone is never
  ownership proof (RUN-01).
- `session_auth.rs` keeps YOLO in process memory only: `set_yolo` has no
  persistence and a new `AuthorizationService` always starts with it
  off. `ApprovalMode::{Interactive, Headless}` is declared by the host.
- `admission.rs` / `fluzo_core::admission` (RUN-05, #112) supplies
  `TaskBudgets`, `BudgetConsumption`, `BudgetDimension`,
  `remaining_active_seconds`. Budget carry-forward uses those types;
  this task does not re-derive budget rules.
- `history.rs` is the precedent for a synchronous facade over the async
  store that never starts an agent, and for deterministic close so the
  WAL index lock is not left dangling.
- Runtime `rustix` features are `fs, process, std`. The control channel
  uses `std::os::unix::net` (std only), so **no new dependency and no
  change to the reviewed feature ceilings** in `scripts/check_dev_setup.py`.
- CLI arg handling is hand-rolled in `main.rs` with a `history`
  subcommand module; a `replay` surface follows the same shape.

Hypotheses to confirm during implementation, not assumed:

- A `read_only(true)` connection against the WAL-mode store opens for the
  same user (the `-shm` file is creatable inside the 0700 drive). If it
  does not, the read-only guarantee falls back to a never-write module plus
  byte-level before/after verification, and the plan records that.

## Decisions, dependencies and risks

- Dependencies are accepted behavior already merged: RUN-01 (#99)
  writer lock and confinement, RUN-02 (#104) policy/grants/YOLO,
  RUN-03 (#111) process lifecycle, RUN-05 (#112) budgets, DAT store
  chain. No new dependency is introduced.
- No agent loop exists (AGT chain), so recovery has no live scheduler to
  hand off to. Resume produces a reconciled *plan and record*, not a
  running task. This is the same boundary RUN-03's dispatch seam has.
- Risk: read-only WAL open semantics (above). Mitigated by an explicit
  probe and a documented fallback.
- Risk: Unix socket path length (about 104 bytes on Linux). Mitigated by
  checking the length and refusing with a clear diagnostic rather than
  truncating.
- Risk: treating a persisted pid as ownership. Mitigated by verifying
  against the live lock and the acquisition nonce, never the pid.
- Excluded: cross-instance coordination, provider rate-limit recovery,
  OS sandbox claims, daemon/service, TUI session browser, automatic
  replay, automatic rollback, forensic erasure.

## Phases

### P1 — Read-only session open and deterministic replay (implemented)

- Outcome: a persisted session can be opened and its timeline replayed
  with a connection that cannot write, without touching providers,
  tools or the writer lock, and with gaps and unknown outcomes visible.
- Owning repository: `fluzo-labs/fluzo`, `fluzo-runtime` plus a
  read-only open primitive in `store.rs`.
- Dependencies: DAT-01 store journal (merged).
- Affected contracts: new `fluzo_runtime::recovery` with
  `RecoveryReader`, `SessionRecord`, `TimelineEntry`, `ReplayGap`,
  `RecoveryError`; new `Store::open_read_only`. No existing signature
  changes.
- Acceptance criteria:
  - Opening a session uses a connection on which any write statement
    fails; a negative test proves SQLite itself refuses.
  - Opening never creates `.fluzodrive/`, never creates the database,
    and never migrates a lower-version store; it refuses instead.
  - Replaying the same persisted events twice yields an identical
    timeline (deterministic reconstruction).
  - An attempt recorded as running with no durable terminal outcome
    renders as `Interrupted`; an operation with intent and no result
    renders as `Unknown`, never as success or failure.
  - A missing range in the event sequence surfaces as a `ReplayGap`
    rather than being silently skipped or reconstructed.
  - Opening and replaying does not acquire the writer lock: a separate
    process can still acquire ownership while a reader is open.
  - The module contains no provider, tool or process-spawn import, so
    dispatch is structurally impossible, not merely unused.
- Actions:
  - [ ] Add `Store::open_read_only` with existence, version and
        safety checks, and probe the WAL read-only behaviour.
  - [ ] Implement `recovery.rs`: typed session view, timeline
        reconstruction, gap and unknown surfacing.
  - [ ] Deterministic fixtures: recorded session with a completed
        attempt, an interrupted attempt, an unknown operation and a
        gap; concurrent writer-lock acquisition.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  recovery` plus the workspace set.
- Actual result: 12 passed / 0 failed. The WAL read-only hypothesis was
  confirmed: `SQLITE_OPEN_READONLY` opens the WAL-mode store for the
  same user and refuses writes with `SQLITE_READONLY`, which now maps to
  the new `StoreError::ReadOnly` instead of collapsing into `Io`.
- Risks and recovery: additive module; no fallback needed.

### P2 — Resume reconciliation with budget carry-forward

- Outcome: an explicit resume revalidates workspace identity and
  recorded revisions, converts running attempts to `Interrupted`, keeps
  unknown outcomes unknown, carries known budget forward, drops
  interrupted-attempt approvals, never restores YOLO, and records a new
  linked attempt.
- Owning repository: `fluzo-labs/fluzo`, `fluzo-runtime`.
- Dependencies: P1 for the read-only view, RUN-01 for identity,
  RUN-02 for grants/YOLO, RUN-05 for budget types.
- Affected contracts: new `fluzo_runtime::resume` with `ResumeRequest`,
  `Reconciliation`, `ResumePlan`, `ReconcileBlock`, `StaleClaim`,
  `BudgetCarryForward`. No existing signature changes.
- Acceptance criteria:
  - Resume without explicit confirmation returns `ConfirmationRequired`
    and writes nothing.
  - A workspace fingerprint mismatch blocks continuation with the
    recorded and observed fingerprints; the current startup directory
    is never silently substituted.
  - A moved Git revision invalidates recorded verification claims and
    edit approvals and surfaces each as a `StaleClaim`.
  - Attempts recorded as running become `Interrupted`; operations with
    intent and no durable result become `OutcomeUnknown` and are not
    replayed.
  - Pending approvals and one-time grants from the interrupted attempt
    do not carry over: a dispatch of one after resume is refused.
  - Standing grants remain subject to current policy and scope rather
    than being auto-applied.
  - Known budget consumption carries forward exactly; uncertain usage
    is reported as uncertain; any extension is an explicit field, never
    implicit.
  - The resumed authorization service has YOLO off even when the prior
    session ran with it on, and no persisted YOLO value is read.
  - Reconciliation is computed read-only; only applying it writes, and
    the reconciliation record plus the new attempt commit atomically.
  - Exactly one new attempt is created, linked to the original task,
    session and prior attempt.
- Actions:
  - [ ] Implement `resume.rs` reconciliation over the P1 view.
  - [ ] DAT-style deterministic crash fixtures: crash after intent
        commit before dispatch, crash after side effect before result
        commit, running attempt, budget carry-forward, grant
        non-carryover, YOLO not restored, workspace mismatch, revision
        drift.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  resume` plus the workspace set.
- Risks and recovery: reconciliation must not double-apply; the apply
  step is idempotent per session and re-running it reports the existing
  resume rather than creating a second attempt.

### P3 — Private owner-verified task-control channel

- Outcome: a separate process can inspect, cancel or force-stop work
  through a private Unix socket bound only by the writer-lock holder,
  verified against the live owner record, with confirmation required
  for force stop and no capability to become a second writer or restore
  YOLO.
- Owning repository: `fluzo-labs/fluzo`, `fluzo-runtime`.
- Dependencies: P2 for the reconciliation result, RUN-01 for the
  writer lock and owner record.
- Affected contracts: new `fluzo_runtime::control` with
  `ControlServer`, `ControlClient`, `ControlRequest` (closed set),
  `ControlResponse`, `ControlError`, `PROTOCOL_VERSION`. No existing
  signature changes.
- Acceptance criteria:
  - The server binds only while holding writer ownership, at
    `.fluzodrive/control.sock`, mode 0600, and refuses a symlinked
    socket path.
  - Every request carries the expected owner token; a token that does
    not match the live `OwnerRecord.owner`, or an ownership check that
    fails, is refused as stale rather than served.
  - The request set is closed at the type level: there is no variant
    that enables YOLO, acquires writer ownership, resumes a task or
    dispatches work. An unknown variant fails closed.
  - A protocol version mismatch is refused before any handler runs.
  - Frames are length-bounded; an oversized or truncated frame is
    refused without allocating proportionally to the attacker's input.
  - `ForceStop` without `confirmed: true` returns
    `confirmation_required` and signals nothing; a sentinel proves no
    process was signalled.
  - An inspection client cannot take the writer lock: a second
    `WriterOwnership::acquire` from the client role still fails with
    `HeldBy`, and the owner keeps its lock throughout.
  - No credential, endpoint or session payload crosses the channel in
    a way that reaches logs; error text is redacted.
  - Fixtures use real Unix sockets and child processes as roles with
    pipe barriers; no sleep-based synchronisation.
- Actions:
  - [ ] Implement `control.rs` framed protocol, server and client.
  - [ ] Owner verification against the live lock and acquisition nonce.
  - [ ] Child-role fixtures: owner, inspection client, control client;
        stale-owner, symlink, oversize, unknown-variant and
        unconfirmed-force-stop negatives.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  control` plus the workspace set.
- Risks and recovery: the socket is same-user reachable and is not a
  security boundary against a cooperating local user; documented as
  such, consistent with the RUN-01 confinement boundary.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | `cargo test -p fluzo-runtime --lib --locked --offline recovery`: 12 passed / 0 failed. Full runtime lib: 413 passed / 0 failed / 8 ignored. |
| P2 | pending | not_run | pending | none yet |
| P3 | pending | not_run | pending | none yet |

## Next step

Execute P1: the read-only open and replay path is the base both the
resume phase and the control channel read through, and it is the phase
that makes "open/replay is read-only" provable rather than asserted.

## Evidence and tracking

- Parent issue: <https://github.com/fluzo-labs/fluzo/issues/18>.
- Ground truth merged: #99 (RUN-01), #104 (RUN-02), #111 (RUN-03),
  #112 (RUN-05).
- GitHub issues remain the state authority; this document is a design
  snapshot, not a parallel board.
