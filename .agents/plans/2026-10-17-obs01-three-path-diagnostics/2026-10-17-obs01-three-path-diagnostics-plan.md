# OBS-01: three-path diagnostics (OBS-01 #13) — plan

Status: proposed -> executing. Nothing in this plan is accepted; acceptance
is maintainer review on the PR.

## Contract

Issue: fluzo-labs/fluzo#13. Baseline: PRD 44, Architecture A09 and 13.1-13.4
at fluzo-docs `60c5b0732fb710cdf705476cee8d9156a5ecd971`; GenAI conventions
pinned at `8ffdf568e1b4391a99adb081db16e8102e36918e`.

Acceptance criteria (issue):
1. SQLite audit is separate from bounded JSONL/OTLP sinks.
2. All signals and token accounting match A09.
3. Privacy (redaction before persistence/export), drops and failed-sink
   responsiveness are tested deterministically.
4. Evidence and documentation linked from the PR.

## Dependency decision (recorded before implementation)

OTLP export is DEFERRED in this increment. The offline cargo cache contains
`opentelemetry 0.20.0`, `opentelemetry-otlp 0.13.0`, `opentelemetry-proto 0.3.0`
and `tracing-opentelemetry 0.21.0`, but:

- every OTLP **HTTP/protobuf** client option of `opentelemetry-otlp 0.13.0`
  (`http-proto`, `reqwest-client`, `reqwest-blocking-client`) requires
  `opentelemetry-http`, which is **not in the offline cache**;
- the cached version's OTLP **metrics** feature is gRPC-only (`metrics` pulls
  `grpc-tonic`), and gRPC is not the accepted initial transport (A09:
  OTLP HTTP/protobuf; "Other transports are not implicitly implemented by
  selecting this initial path").

Per the task dependency policy: no network fetch, no hand-vendoring.
`OTLP deferred: opentelemetry-http not in offline cache`. A follow-up child
issue under #13 carries the OTLP HTTP/protobuf export path, the local
receiver conformance fixtures (13.3/13.4) and the Compose verification.
The hub is built with a sink boundary so the OTLP exporter plugs in later
without redesign.

## Scope (this increment)

### fluzo-core `telemetry.rs` (pure DTOs + policy, no I/O)

- `TELEMETRY_FORMAT_VERSION = 1`; `GENAI_CONVENTIONS_REVISION` constant.
- `DiagnosticRecord` envelope: format version, `SignalKind` (Log/Span/Metric),
  timestamp, `ResourceIdentity`, optional `TraceRef`, typed payload
  (`LogPayload` / `SpanRecord` / `MetricSnapshot`).
- `MetricSnapshot` carries instrument name, unit, kind (Counter/Histogram),
  aggregation window and SDK-style aggregate — documented envelope fields per
  13.2, not per-token samples.
- GenAI types per 13.3: `GenAiUsage` with Option-valued input/output tokens and
  cache-read/cache-write/reasoning subsets (omitted when unknown, never zero);
  pinned instrument-name and unit constants; `UsageLedgerEntry` distinguishing
  `FinalResponse`, `PhysicalRetry` and `Incomplete` provenance.
- `ProviderUsageReport` + `normalize_usage`: pure once-per-operation
  normalization — repeated cumulative chunks deduplicated against the last
  observed cumulative, usage-only final chunk honored, subsets never summed on
  top of totals, unknown values omitted rather than zeroed.
- `SinkDropReason` (QueueFull / RecordTooLarge / WriteFailed / ShutdownStopped)
  and `SinkHealth` (queue depth records+bytes, capacities, dropped-by-reason,
  last error category, degraded flag).
- `AuditRecord` + `AuditCategory` (PermissionDecision / IntentCommit /
  OutcomeCommit / ConfigurationChange) with correlation id.

### fluzo-runtime `telemetry.rs` (hub, privacy, health, shutdown)

- `DiagnosticHub`: fan-out with one bounded queue + worker thread per sink
  (count AND byte limits, per-record byte bound). `publish` is non-blocking
  `try_send`; refusal never blocks the producer and is counted per sink with a
  reason. One saturated/failing sink cannot stall another.
- Redaction at the capture boundary before fan-out: structural exclusion of
  known credential-shaped attribute keys and scrubbing of caller-supplied
  secret values. Both sinks only ever see redacted records.
- `health()` per-sink snapshot from atomic counters; degraded state on write
  failure; counters are in-memory and never claimed durable.
- `shutdown(deadline)`: stops admission immediately, drains within the
  deadline, reports undrained records as `ShutdownStopped` drops. Failure of
  a sink never emits new records through the same pipeline (no recursive
  telemetry storm).

### fluzo-runtime `telemetry_archive.rs` (bounded JSONL local archive)

- JSONL segments under `.fluzodrive/telemetry/` (segment rotation by byte
  bound, `0600` files, drive-anchored like DAT-01 discipline).
- Batched writes with fsync off the producer path; write failure marks the
  sink degraded, counts `WriteFailed`, and does not kill the pipeline.
- Reader reports incomplete trailing records after a crash instead of
  presenting them as committed; never deletes prior segments.

### fluzo-runtime `telemetry_audit.rs` (required SQLite audit)

- Schema v3: `audit` table in the DAT-01 bootstrap unit (SCHEMA_VERSION 2->3).
- `AuditTrail::commit` writes audit records inside a `BEGIN IMMEDIATE` unit
  together with any dependent state/event updates. A failed mandatory audit
  commit returns an error and the dependent effect must not proceed
  (A09 audit gate).
- Separation proofs: the diagnostic hub has no store handle; diagnostics-only
  runs leave zero audit rows; audit failure never disables or substitutes
  diagnostics, and diagnostic loss never relaxes `synchronous=FULL`.

### Explicitly out of this increment

- OTLP export, local OTLP receiver conformance, Compose stack verification
  (deferred child issue; crate not cached).
- Task/attempt/operation state machines and real provider gateways (EXEC /
  provider work); audit and GenAI contracts are exercised with fixtures.
- `tracing` bridge wiring into the hub (arrives with the OTLP increment so
  the three-signal bridge is tested together).
- TUI/CLI surfaces; no per-frame or per-token emission anywhere.

## Acceptance-criteria mapping

| AC | Evidence planned |
| --- | --- |
| 1 audit separate from bounded sinks | `audit_commit_gate_blocks_dependent_effect_on_audit_failure`; `diagnostics_never_substitute_for_audit_records`; hub holds no store handle (compile-time separation); audit failure leaves diagnostic flow green in `audit_failure_does_not_disable_diagnostic_sinks` |
| 2 signals + token accounting match A09/13.3 | core pure tests: dedup of repeated cumulative usage, usage-only final chunk, missing counts omitted not zeroed, cache/reasoning subsets never summed, retry vs final ledger provenance, pinned instrument names/units, `TokenModality`, `MetricTemporality`, span-name pattern; envelope carries all three signal kinds and round-trips through the archive |
| 3 privacy/drops/failed-sink tested | redaction-before-persistence (read archive file back), oversized-record refusal, queue-full refusal without blocking producer, slow-sink isolation, write-failure degraded state, shutdown deadline drops, no recursive failure storm — all deterministic, no sleeps as synchronization beyond bounded polling |
| 4 evidence + docs | OBSERVABILITY.md (paths, envelope, drop semantics, health, OTLP deferral), AGENTS.md status line, this plan, PR links |

## Verification

Pinned toolchain, `--locked --offline`:
- `cargo fmt --all -- --check`
- `cargo test --workspace --locked --offline`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- SIM-01 network fixtures: not applicable this increment (no network sink;
  OTLP receiver tests deferred with the child issue).
- Live inference: `not_run`.

## Progress

| Phase | Implemented | Verification | Review |
| --- | --- | --- | --- |
| P1 core telemetry DTOs + normalization | done | 15 core `telemetry` tests pass | pending |
| P2 runtime hub + archive + audit | done | 17 runtime `telemetry` tests pass | pending |
| P3 tests + docs + OTLP child issue | done | full suite green x3, 76 python tests, graph check clean | pending |

### Work completed in this session (continuation of an orphaned WIP)

The branch arrived with commit `9402f40` ("wip") left mid-flight by a
worker that lost its context. State found and what was done:

1. **Fixed the failing `queue_full` test.** Both the blocked and the
   healthy sink were unnamed `MemorySink::default()`, so the per-name
   health lookup resolved ambiguously and the degraded-state assertion
   could not distinguish them. Rewrote with named sinks
   (`MemorySink::new("blocked")` / `"healthy"`) plus `blocked()` and
   `failing()` constructors. Also corrected a `u64`/`usize` mismatch.
   Now deterministic across repeated isolated runs.
2. **Clippy clean.** Collapsed nested `if let` in the archive into a
   `segment_index_of` helper; replaced `vec![]` and `&[x.clone()]`
   idioms; fixed a `Self::` reference in a free-function context.
3. **Added the A09/13.3 gaps the WIP had not reached:** `MetricTemporality`
   on `MetricSnapshot`, the pinned `attributes` module with
   `ABSENT_BY_DEFAULT`, `TokenModality`, `gen_ai_span_name`, and
   retry-vs-final accounting on `UsageNormalizer`
   (`report_retry` / `total_known`).
4. **Found and fixed a real defect in the default redaction policy.** The
   broad `token` forbidden substring also matched the pinned GenAI keys
   `gen_ai.token.modality` and `gen_ai.usage.*_tokens`, so the default
   policy silently rewrote the token accounting to `[REDACTED]` while
   every test that only checked for leaked secrets still passed. Added
   `RedactionPolicy::known_safe_keys`, an exact-match exemption derived
   from the core vocabulary constants, and
   `default_policy_preserves_pinned_gen_ai_usage_attributes` which pins
   both sides: pinned GenAI attributes survive, arbitrary
   credential-shaped keys are still caught.
5. **Added cross-arm redaction coverage.** The WIP tested log attributes
   only. Span attributes and metric dimensions are separate maps, so a
   log-only policy would leak through them; now covered by
   `redaction_covers_span_attributes_and_metric_dimensions_not_just_logs`
   against bytes actually written to disk.
6. **Added archive envelope round-trip** proving instrument type, unit,
   window and temporality survive serialization
   (`metric_envelope_round_trips_through_the_archive_with_temporality`).
7. **Verified the OTLP deferral rather than assuming it.** The cached
   `opentelemetry-otlp 0.13.0` offers no HTTP path that can be built
   offline: `http-proto`, `reqwest-client`, `reqwest-blocking-client`
   and `surf-client` all require `opentelemetry-http`, which is not in
   the cache, and the `metrics` feature pulls `grpc-tonic`. Filed as
   child issue #88 rather than substituting gRPC for the accepted
   transport.
8. **Wrote OBSERVABILITY.md** and the AGENTS.md status line.

### Verification actually run

```
cargo fmt --all -- --check                      pass
cargo clippy --workspace --all-targets \
  --locked --offline -- -D warnings           pass
cargo test --workspace --locked --offline       pass (357 tests)
  repeated x3                                  0 failures each
cargo test -p fluzo-core --locked --offline telemetry     15 pass
cargo test -p fluzo-runtime --locked --offline telemetry  17 pass
python3 scripts/check_dev_setup.py              pass (no new deps)
python3 -B -m unittest discover -s scripts -p 'test_*.py'
                                              76 pass
```

No production dependency was added or changed; the dependency graph
checker passes unchanged, which is the check that matters for the
"no silent broadening" rule.

## Next step

Human review. Nothing in this plan is accepted. Open items for the
reviewer:

- The redaction/`token` defect (item 4) is a correctness finding in code
  that previously looked green. Worth a separate look at whether any other
  pinned vocabulary collides with a forbidden substring.
- Total telemetry archive size is bounded per segment only; segment
  count grows until DAT-02e (#76) lands. Deliberately not described as
  capped.
- OTLP export remains unimplemented (#88). No claim of three-signal
  end-to-end delivery is made anywhere in this increment.

## Next step

Execute P1-P3, then commit/push/PR `Ref: #13`. Merging must not close #13.
