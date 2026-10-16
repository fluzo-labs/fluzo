# RUN-05: Task budgets and per-process inference admission

Authority: PRD v0.2 sections 43.1-43.3 and Architecture A04 in
`fluzo-docs` revision `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
Implements issue #19.

## Scope split

- `fluzo-core::admission` holds the **pure rules**: pool limits
  validation, the FIFO/shadow admission decision function, task budget
  dimensions and exhaustion, the token reservation rule and provider
  rejection classification. No I/O, no clocks, no state.
- `fluzo-runtime::admission` holds the **live controller**: one
  `AdmissionController` per runtime process with per-pool counters,
  per-model capacity-identity counters, a FIFO foreground queue,
  uncertain-slot bookkeeping and bounded views.

## Pool and queue rules (PRD 43.3)

- Pools are named per inference server/resource group and configured
  from the typed `CapacityPoolSettings` registry
  (`max_in_flight`, `foreground_reserved_slots`, `queue_capacity`,
  `queue_timeout_seconds`). Defaults: 1 / 1 / 8 / 30s.
- The shadow allowance is `max_in_flight - foreground_reserved_slots`.
  With the default one-slot pool the allowance is zero, so Laya shadow
  observations are **skipped**, never queued. A shadow request never
  queues behind foreground work and never competes for a reserved slot.
- Foreground classes (agent turn, compaction, setup probe, live test)
  are FIFO-equal. A new arrival never overtakes a queued request, even
  when pool capacity appears free or the arrival uses a different
  model. Strict FIFO can hold the queue head behind its own model
  ceiling; the pool does not reorder waiting work.
- Queue overflow is a visible typed denial (`capacity_exhausted`), not
  a silent drop or an unbounded wait.
- Every queued wait has a deadline: the pool queue timeout bounded by
  the task's remaining active time. `expire(now)` releases expired
  waiters; an expired waiter never dispatches.
- Queue duration is reported separately from inference time
  (`PoolView::oldest_queue_wait_ms`).

## Capacity identities and aliases

Model limits bind to `ModelId`, an explicit capacity identity. Aliases
of the same backend model must resolve to one `ModelId` upstream so
they share one local allowance instead of multiplying it. A model
ceiling blocks its requests even while the pool has headroom.

## Uncertain slots (PRD 43.3, "dropping is not stopping")

A dispatched request that times out or disconnects with unconfirmed
remote completion is released as `Uncertain`. The slot keeps
consuming pool and model capacity until an explicit
`reconcile(slot, evidence)` with confirmed completion or
cancellation. There is no timer-based auto-free, no RAII release on
future drop, and no automatic operation replay.

## Budgets (PRD 43.1)

Task budgets map from the typed `HarnessSettings` registry: turns,
active seconds, tool calls, total tokens and an optional monetary
ceiling. Exhaustion is deterministic (fixed dimension order),
machine-readable (`budget_exhausted` with dimension/limit/consumed)
and is checked **before** capacity is consumed. A request whose
`input estimate + max output reservation` would reach the token
budget is denied before dispatch. Absent monetary data is not zero
consumption.

## 429 and redirect semantics (PRD 43.2)

`classify_status` types a 429 as `RateLimited` and redirects/fatal
client errors as non-retry-eligible. There is no automatic retry,
cooldown, fallback or redirect-follow anywhere in this path. The
simulator integration tests prove a 429 sends exactly one request
and that any retry-like second request is caught as unexpected
traffic by the strict fixture.

## Cross-process rule (A04)

Two controllers with the same pool name keep fully independent
counters. This process never coordinates server-side quotas with
another instance; oversubscription across instances is the operator's
responsibility, not a coordination failure. There is no shared
journal, lock or peer discovery.

## No bypass

There is no parameter that skips admission. YOLO (RUN-02) removes
prompts, not budgets, pool limits, workspace checks or cancellation,
and this module has no knowledge of prompts.

## Verification

```sh
cargo test -p fluzo-core --locked --offline admission
cargo test -p fluzo-runtime --lib --locked --offline admission
cargo test -p fluzo-runtime --test admission_simulator --locked --offline
```

The simulator tests use the existing strict loopback HTTP fixture
(`tests/support`): unexpected requests, missing steps and byte caps
fail the run. No live inference, no external network.

## Remaining gaps

- The agent execution loop that *calls* this controller is not
  implemented yet; today the contract is exercised by unit and
  simulator-level tests, not by a running task.
- Monetary budget consumption depends on provider cost reporting that
  does not exist yet; the dimension stays `None` until then.
