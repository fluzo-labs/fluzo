# Laya shadow observations (AGT-03)

`fluzo-runtime::shadow` implements the optional, replaceable Laya
shadow capability: a non-blocking, observation-only adapter that
asks typed questions (`choice`, `noul`, `score`) over
`POST /v1/systemone` and records every outcome explicitly.
Recommendations are data with provenance — they never authorize,
route or execute anything (PRD 10.1, Architecture A04). The
feature is disabled by default and the core has no dependency on
Laya: replacing the model or provider means supplying another
`DecisionObserver`, with no coordinator, UI or protocol change
(Architecture A10).

## Modules

- `wire.rs` — pure `/v1/systemone` protocol: `ShadowQuestion`
  (validated: non-empty prompt, 2..=8 unique options, size
  bounds), `build_request_json` (schema_version 1, observation
  id, decision point, question) and `parse_answer` with strict
  validation: schema version, expected type match, answer inside
  the candidate set, probabilities in `[0, 1]` summing to at
  most 1 and keyed only by candidates, score answers covering
  every candidate, confidence and `act_probability` in bounds,
  response byte ceiling. Every rejection is a typed
  `AnswerRejection`, never a panic or a silent default.
- `mod.rs` — the replacement boundary. `DecisionObserver` is a
  boxed-future trait over std-only types; `ShadowRequest` carries
  the decision point and bounded context estimates;
  `ShadowObservationRecord` is the PRD 44.4 evaluation record
  (question, candidates, answer, recommended action, actual
  action left to the caller, status, sizes). `ObservationSink`
  receives every terminal record.
- `observer.rs` — `LayaShadowObserver`, the `/v1/systemone`
  client. Reuses the AGT-01 gateway-owned gates: endpoint parse,
  DNS resolve-then-classify, consent before any socket, bearer
  credential reference resolution (value only enters a sensitive
  header, never an error or telemetry field), admission with the
  `ShadowObservation` skip-not-queue rules, one bounded deadline
  and no retries. Telemetry: one client span plus duration
  histogram per observation, purpose `shadow_observation`,
  unsampled.
- `coordinator.rs` — `ShadowCoordinator`: enable/disable,
  per-task observation budget, decision-point deduplication,
  bounded queue (overflow dropped, never grown), one bounded
  worker with a concurrency semaphore, automatic pause after
  consecutive failures and explicit `resume()`. `submit()` is
  non-blocking: it returns the admission decision of this layer
  immediately and never gates the caller.

## Enablement and pause rules

- Disabled by default. `decision.enabled` (settings) maps to
  `ShadowCoordinator::set_enabled`; a disabled coordinator
  rejects submits with `Disabled` and records nothing.
- Automatic pause: `failure_pause_threshold` consecutive
  failures pause new observations with `Paused { reason:
  ConsecutiveFailures }`. Only `Rejected`, `Timeout` and
  transport/protocol `Failed` outcomes count; consent denials,
  admission skips, cancellations and configuration failures
  never feed the streak. A success resets it.
- The pause survives disable/re-enable. Only an explicit
  `resume()` (operator action) clears it.
- Disabling drops queued observations (recorded as
  `cancelled_queued_dropped`) and signals the in-flight one
  best-effort (`cancelled_after_dispatch`).

## Failure semantics (all typed, machine-readable via `code()`)

| Event | Record status | Admission slot | Retry |
| --- | --- | --- | --- |
| Valid answer | `Completed` | released | — |
| Invalid answer (type/set/probability/schema) | `Rejected { reason }` | released | never |
| 3xx response | `Failed: redirect_blocked`, Location never contacted | released | never |
| 429 / quota | `Failed: rate_limited` | released | never |
| Deadline exceeded | `Timeout` | `Uncertain` if dispatched | never |
| Transport break after dispatch | `Failed: transport` | `Uncertain` | never |
| Cancelled before dispatch | `Cancelled: before_dispatch` | released | — |
| Cancelled after dispatch | `Cancelled: after_dispatch` | `Uncertain` | — |
| Consent denied | `Failed: consent_denied`, zero sockets | not taken | — |
| No shadow allowance / foreground queued | `Skipped { reason }` | not taken | — |
| Response over ceiling | `Failed: response_too_large` | released | never |
| Credential missing/invalid | `Failed: credential_*` | not taken | — |

Once the request body is handed to the transport, a timeout or
cancellation does not prove the provider never saw it: the slot
stays `Uncertain` until explicit reconciliation, exactly as in
the chat gateway. A skipped observation sends zero packets —
proven against the strict fixture.

## Non-influence guarantees

- Nothing in `fluzo-core` or the execution path imports
  `shadow`; the only coupling is the shared admission controller
  and diagnostic hub, both consumed as capabilities.
- `ShadowObservationRecord.actual_action` is filled by the
  caller after the real decision; the record documents the
  recommendation alongside what actually happened, it cannot
  become a grant (RUN-02 grant matching never sees it).
- `DecisionObserver` results reach the app only through
  `ObservationSink` records. There is no API by which an answer
  widens permissions, changes routing or triggers execution.

## Verification

- Unit: `cargo test -p fluzo-runtime --lib --locked --offline
  shadow` — wire validation matrix, record/status codes,
  coordinator budget/dedup/queue/pause/resume/disable,
  substitute-observer flow, observer gate ordering with stub
  consent/credentials/admission (37 tests).
- Simulator: `cargo test -p fluzo-runtime --test
  laya_shadow_simulator --locked --offline` — strict loopback
  fixtures for round trips (choice/noul/score), unknown-type and
  out-of-set rejection without retry, redirect blocked with zero
  traffic to the target, 429 without retry, timeout and
  cancellation leaving the slot uncertain, consent denial and
  skip paths with zero requests, response ceiling, telemetry
  without credential leakage, coordinator end-to-end with the
  real observer (14 tests).
- Live inference: `not_run`. Deterministic loopback fixtures
  only; no real Laya endpoint was contacted.

## Not implemented here

- Agent-loop call sites: nothing submits decision points yet
  (AGT-04+). The coordinator is ready to be wired.
- Settings UI for `decision.*` beyond the existing typed keys.
- Actual-action correlation reporting and shadow-vs-primary
  dashboards.
