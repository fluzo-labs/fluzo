# T31: RUN-05 task budgets and per-process inference admission — plan

Status: implemented and verified; PR https://github.com/fluzo-labs/fluzo/pull/112
opened with Ref: #19 (human review pending).
Ref: #19. PRD 43.1/43.2/43.3 and Architecture A04 at fluzo-docs
revision 60c5b0732fb710cdf705476cee8d9156a5ecd971.

## Contract

Task budget rules and per-process model/pool admission. Pure rules live in
`fluzo-core::admission`; the live per-process controller lives in
`fluzo-runtime::admission`. No cross-instance coordination (A04), no
provider rate-limit recovery, no daemon. The existing `quota.rs` is the
DAT-02b *storage* ledger and stays untouched; this is the inference
admission layer.

Key rules to encode:

- Atomic admit only when both the model capacity identity and the pool
  have capacity; every request class shares the pool (agent turns,
  compaction, setup probes, live tests, shadow observations).
- Foreground = agent turn, compaction, setup probe, live test. Shadow =
  Laya observation. Shadow is skipped (not queued) while any foreground
  request is queued or the shadow allowance
  (`max_in_flight - foreground_reserved_slots`) is exhausted.
- FIFO fairness among foreground waiters; queue capacity 8 and 30 s
  deadline by default, bounded by remaining active-task time supplied by
  the caller. Full queue → `capacity_exhausted`; expired wait →
  `capacity_wait_timeout`. No silent model switch, no retry loop.
- Cancelled queued requests are removed without dispatch.
- Post-dispatch timeout/disconnect marks the slot uncertain: it keeps
  occupying pool and model capacity until explicit reconcile evidence.
- Lowering a limit drains: existing requests keep running, new admission
  is blocked while occupied >= the new limit.
- Aliases of one backend model share one capacity identity.
- HTTP 429 / recognized capacity rejection: typed outcome, never an
  automatic retry, cooldown or fallback.
- Task budgets (turns, active seconds, tool calls, total tokens,
  optional cost) are checked before admission; exhaustion is a typed
  `budget_exhausted { dimension, limit, consumed }` outcome. Before a
  request, input estimate + max response output must fit the remaining
  token budget.

## Phases

### P1 — core pure rules (implemented)

`crates/fluzo-core/src/admission.rs`:

- `RequestClass` (foreground/shadow split), `ShadowSkipReason`.
- `PoolLimits` validated from `CapacityPoolSettings` fields;
  `PoolSnapshot` and pure `decide_admission(class, model_at_limit, snapshot)`.
- `BudgetDimension`, `TaskBudgets` (from HarnessSettings), pure
  `check_budgets`, `reserve_inference_tokens` (estimate + max output
  must fit remaining), `BudgetExhausted`.
- `ProviderRejection` classification: 429 → `RateLimited` (never
  retry-eligible, `Retry-After` ignored), recognized capacity rejection
  → `CapacityRejected`, 5xx/IO → `Transient` (bounded retries only),
  other 4xx → `Fatal`.
- Typed `AdmissionDecision`/`AdmissionDenied` including
  `CapacityExhausted`, `CapacityWaitTimeout`, `ShadowSkipped`,
  `BudgetExhausted`.
- Unit tests: decision matrix (capacity, queue fairness, reservation,
  shadow skips, queue full), budget exhaustion per dimension, token
  reservation boundary, rejection classification, limits validation.

### P2 — runtime per-process controller (implemented)

`crates/fluzo-runtime/src/admission.rs`:

- `AdmissionController`: pools keyed by `PoolId`, model capacity
  identities per pool, FIFO queues, injectable monotonic clock (no
  tokio test-util needed; time is passed to every call).
- `try_admit(request, now) -> Admission` (never blocks; Queued carries
  ticket + position + deadline bounded by caller-supplied remaining
  task time), `admit_next(pool, now)` (FIFO handoff on freed capacity),
  `expire(now)`, `cancel_queued(ticket)`, `mark_dispatched(ticket)`,
  `release(slot, Completed|Uncertain)`, `reconcile(slot, evidence)`,
  `apply_limits(pool, limits)` (drain semantics), `view(pool)` with
  occupied/uncertain/queued counts and per-ticket queue time.
- Queue time and inference time reported separately.
- Unit tests with controlled clock: fairness, deadline bound by task
  time, cancel-without-dispatch, uncertain occupancy until reconcile,
  reduced-limit drain, alias identity sharing, shadow skip at default
  one-slot pool, YOLO flag irrelevant (no bypass path exists).

### P3 — simulator integration + docs (implemented)

- `crates/fluzo-runtime/tests/admission_simulator.rs` using the
  hardened loopback fixture (#81/#94 discipline): real overlapping
  HTTP through the controller with gated streaming.
  - pool ceiling bounds server-observed peak overlap
  - 429 with `Retry-After` → exactly one request, typed
    `RateLimited`, no retry/cooldown/fallback
  - queued-then-cancelled request never reaches the server
  - queue timeout never dispatches
  - uncertain slot (mid-stream disconnect) blocks new admission until
    explicit reconcile; server sees no extra request
  - two independent controllers with the same pool name each admit
    their own ceiling (no arbitration; aggregate overlap equals the
    sum, proving independence)
- `ADMISSION.md`: contract, typed outcomes, PRD/A04 mapping, verified
  commands, remaining gaps.
- `AGENTS.md`: add focused verification commands.

## Verification (commands to run and record)

- `cargo test -p fluzo-core --locked --offline admission`
- `cargo test -p fluzo-runtime --lib --locked --offline admission`
- `cargo test -p fluzo-runtime --test admission_simulator --locked --offline`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `cargo test --workspace --locked --offline`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- Live inference: `not_run`

## Non-goals

No provider gateway/HTTP client integration beyond the test driver, no
task harness, no TUI cards, no cross-instance coordinator, no rate-limit
scheduler, no changes to storage quota, no new dependencies.
