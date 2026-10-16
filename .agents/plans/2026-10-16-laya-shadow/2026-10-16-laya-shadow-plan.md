---
name: "2026-10-16-laya-shadow"
description: "AGT-03 optional replaceable Laya shadow capability implemented, verified and delivered as a Ref: #22 PR"
created_at: "2026-10-16T13:55:20Z"
has_completed_all_phases: false
---

# Optional replaceable Laya shadow capability (AGT-03)

## Source and revision

- Plan identifier: fluzo-agt03-laya-shadow-v1.
- Source: fluzo-labs/fluzo issue #22 (read via `gh` on 2026-10-16) and worker task T36.
- Reviewed revision: issue #22 body (seed AGT-03, M5, providers area).
- Reference repository and branch: fluzo-labs/fluzo, `feat/agt03-laya-shadow` at `4166767` (includes merged AGT-01 #114).
- Design baseline: PRD v0.2 sections 10.1, 43.3, 44.4 and Architecture v0.1 decisions A04/A10 at fluzo-docs revision `60c5b0732fb710cdf705476cee8d9156a5ecd971` (fetched and read 2026-10-16).
- Approval: worker-task delegation (T36) for implementation, verification, commit, push and a non-closing PR. Human review remains a gate.

## Goal

A runtime shadow-observation capability that sends bounded typed questions
(`choice`, `noul`, `score`) to a Laya `POST /v1/systemone` endpoint,
records every outcome explicitly (completed, rejected, skipped, timed
out, cancelled), never lets a recommendation influence execution, is
disabled by default, pauses automatically after consecutive failures and
can be replaced through a narrow typed observer trait without touching
coordinator or UI code (A10).

## Scope and exclusions

In scope:

- `fluzo-runtime::shadow` module: wire types + strict response
  validation, `DecisionObserver` trait, `LayaShadowObserver` HTTP
  adapter (consent + admission + timeout, no retries), `ShadowCoordinator`
  (enable/disable, failure pause, per-task budget, decision-point
  dedup, non-blocking submit, status snapshot, record sink).
- Reuse of AGT-01 gateway components: endpoint parsing, `classify`,
  `ConsentGate`/`AcceptedEndpoints`, `CredentialSource`, admission
  `ShadowObservation` skip semantics, `DiagnosticHub` telemetry.
- Deterministic loopback simulator tests + pure unit tests.
- `LAYA_SHADOW.md` documentation and AGENTS.md status paragraph.

Excluded (documented follow-ups, not silently widened):

- Durable SQLite persistence of 44.4 evaluation records (DAT child
  issue; this increment emits records through the diagnostic pipeline
  and an injectable sink).
- TUI/devmenu wiring of `decision.enabled` effective-state cards (UI
  story; the coordinator exposes the status snapshot it needs).
- Active routing, promotion logic, calibrated thresholds (post-MVP).
- Settings-registry changes: `decision.*` keys already exist; no new
  settings are added.

## Verified context

- `crates/fluzo-core/src/settings.rs`: `DecisionSettings` exists with
  `enabled=false`, `mode=shadow`, `model`, `timeout_seconds=5`,
  `max_observations_per_task=30`, `failure_pause_threshold=3`;
  `Provider::LayaSystemOne` choice exists.
- `crates/fluzo-core/src/admission.rs`: `RequestClass::ShadowObservation`,
  `ShadowSkipReason`, `shadow_allowance()` skip-not-queue rules exist
  (#112).
- `crates/fluzo-runtime/src/providers/gateway.rs` (#114 merged):
  `chat_target` endpoint parser, `resolve`, `classify` (pub(crate)),
  `ConsentGate`, `AcceptedEndpoints`, `CredentialSource`, slot
  release semantics (`Completed` vs `Uncertain`), `DiagnosticHub`
  publish patterns.
- `crates/fluzo-runtime/tests/provider_gateway_simulator.rs` and
  `tests/support/`: strict loopback fixture discipline (#81/#94).
- Hypothesis to confirm during implementation: `chat_target`/`ChatTarget`
  and the bearer-header builder are private to `gateway.rs` and must be
  widened to `pub(crate)` for reuse instead of duplicating the parser.

## Decisions, dependencies and risks

- Dependencies are accepted behavior: AGT-01 (#114 merged), RUN-05
  (#112 merged), consent (#110 merged).
- Decision: the shadow adapter uses its own HTTP path to
  `/v1/systemone` (PRD: Laya is not a Chat Completions provider) but
  composes the same gateway-owned gates (consent, admission,
  credentials, telemetry), satisfying "not a private HTTP path that
  bypasses limits" (Architecture A10 note).
- Decision: replacement boundary is `DecisionObserver` returning a
  boxed future (std-only; no new dependencies such as `async_trait` or
  `futures`).
- Risk: the real Laya wire schema is not documented publicly beyond
  probes; the wire contract is versioned (`schema_version: 1`) and
  owned here so a substitute or a corrected schema is a localized
  change. Recorded as an assumption, not a claim of upstream fidelity.
- Risk: none of this is wired into a live agent loop yet (no agent loop
  exists); the capability is exercised through its own API and
  fixtures.

## Phases

### P1: Shadow capability with simulator-backed contract

- Outcome: `fluzo-runtime::shadow` compiles and passes pure unit
  tests plus a strict loopback `/v1/systemone` simulator suite that
  proves the acceptance criteria; docs updated.
- Owning repository: fluzo (this worktree).
- Dependencies: merged #114, #112, #110.
- Affected contracts: new `shadow` module (wire types,
  `DecisionObserver`, `LayaShadowObserver`, `ShadowCoordinator`,
  `ObservationSink`); `gateway.rs` widened internals (`chat_target`,
  bearer builder) to `pub(crate)` for reuse; no changes to existing
  public behavior.
- Acceptance criteria:
  - AC1: disabled by default; enable/disable and failure pause obey
    consent (denial = zero sockets) and capacity (shadow skip reasons
    from admission; foreground-queued and allowance-exhausted skip
    with zero requests); pause after N consecutive transport/protocol/
    timeout failures blocks new submits and is not lifted by
    re-enable; explicit resume works.
  - AC2: recommendations are inert data; no type path from an answer
    to a grant/permission/tool dispatch; the only output is the
    observation record; late/failed/skipped/cancelled are explicit
    statuses; each request carries the decision point and observed
    state version/fingerprint.
  - AC3: a substitute `DecisionObserver` implementation runs the
    full coordinator flow unchanged (proven with a stub observer).
  - Wire validation: unknown answer types, invalid probabilities,
    out-of-set choices, bad schema version rejected as typed
    rejections with no retry.
  - Redirects blocked with zero traffic to the Location target; 429
    without retry; timeout after dispatch leaves the admission slot
    Uncertain; disable drops queued and cancels in-flight best-effort
    with slots remaining occupied/uncertain.
- Actions:
  - [x] Implement wire types + validation (`shadow/wire.rs`).
  - [x] Implement `DecisionObserver` + `LayaShadowObserver`
        (`shadow/observer.rs`).
  - [x] Implement `ShadowCoordinator` + `ObservationSink`
        (`shadow/coordinator.rs`).
  - [x] Pure unit tests for wire, observer unit behavior and
        coordinator state machine (incl. substitute observer).
  - [x] `tests/laya_shadow_simulator.rs` strict fixture suite.
  - [x] `LAYA_SHADOW.md` + AGENTS.md paragraph.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review (PR `Ref: #22`).
- Verification: pinned toolchain, `--locked --offline`:
  `cargo fmt --all -- --check`; focused `cargo test -p fluzo-runtime
  --lib --locked --offline shadow`; `cargo test -p fluzo-runtime
  --test laya_shadow_simulator --locked --offline`; then
  `cargo test --workspace --locked --offline`,
  `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`,
  `python3 -B -m unittest discover -s scripts -p 'test_*.py'`,
  `python3 scripts/check_dev_setup.py`. Live inference: not_run.
- Risks and recovery: no migrations; module is additive; removal is
  clean detach (optional capability). Wire-schema assumption is
  isolated in `wire.rs`.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | done | passed (2026-10-17, pinned Rust 1.98.0, Linux x86_64) | pending | 37 shadow unit + 14 simulator tests; full workspace green |

## Next step

Human review of the `Ref: #22` PR. No further phase is authorized in
this plan until review feedback lands.

## Evidence and tracking

- Issue: https://github.com/fluzo-labs/fluzo/issues/22
- PR: to be created (Ref: #22).

### Verification results (2026-10-17, pinned toolchain 1.98.0, `--locked --offline`)

- `cargo test -p fluzo-runtime --lib --locked --offline shadow` —
  37 passed (wire matrix 14, mod/status 4, observer 8, coordinator 11
  incl. substitute-observer and disable-drops-queued).
- `cargo test -p fluzo-runtime --test laya_shadow_simulator
  --locked --offline` — 14 passed: choice/noul/score round trips,
  unknown-type and out-of-set rejection with exactly one request,
  redirect blocked with zero traffic to the Location target, 429 with
  no retry, timeout and cancel-after-dispatch leaving the admission
  slot Uncertain, consent denial and skip paths with zero requests,
  response ceiling, telemetry without credential leakage, coordinator
  end-to-end with the real observer.
- `cargo test --workspace --locked --offline` — all green (521 runtime
  lib, 184 TUI, all integration suites, 0 failures).
- `cargo clippy --workspace --all-targets --locked --offline --
  -D warnings` — clean.
- `cargo fmt --all -- --check` — clean.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` — 90
  passed. `python3 scripts/check_dev_setup.py` — resolved graph,
  toolchain, LSP config and 12 skills checked.
- Live inference: `not_run` (deterministic loopback fixtures only).
