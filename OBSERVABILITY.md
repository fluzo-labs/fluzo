# Observability

OBS-01 implements two of the three diagnostic paths required by
Architecture 13.1: the durable SQLite audit path and the bounded local
JSONL archive. OTLP export is deferred to #88. The paths stay separate:
audit failure blocks the dependent effect, diagnostic sink failure only
degrades visibility.

## The three paths

| Path | Storage | Failure semantics | Status |
| --- | --- | --- | --- |
| Audit | `.fluzodrive/state.sqlite3` `audit` table | Blocks the dependent state update | Implemented |
| Local archive | `.fluzodrive/telemetry/telemetry-<n>.jsonl` | Bounded loss per record, visible gap | Implemented |
| OTLP | Configured collector | Degraded, no retry | Deferred to #88 |

Audit is a correctness record. The archive and OTLP are observability and
must never be treated as substitutes for it.

## Audit path

`AuditTrail::commit_with(records, updates, events)` commits audit records
together with their dependent state updates and journal events in one
`BEGIN IMMEDIATE` unit. Any failure — constraint, busy, I/O — discards
the whole unit and returns the error, so the caller must not perform the
authorized effect. `AuditTrail::commit(records)` is the standalone form
for audit-only durability.

Categories are `permission_decision`, `intent_commit`, `outcome_commit`
and `configuration_change`. A failed archive or OTLP write leaves the
audit committed and does not disable the diagnostic sinks; the two
failure classes are deliberately independent so losing visibility never
relaxes `synchronous=FULL` or unblocks an unauthorized effect.

The `audit` table arrives as the explicit `MigrationStep::AddAuditTable`
step from schema v3 to v4 in the DAT-02c migration path. It is not
created by bumping a version marker inline. `MIGRATIONS` stays
contiguous and `TARGET_VERSION` moves with it; the migration tests derive
their contiguity and log-length assertions from those constants, so the
step is covered without further edits.

## Capture boundary

One `DiagnosticHub` owns admission, redaction, fan-out and loss
accounting. Producers call `publish` and never reach a sink directly.

- **Bounded queues.** `HubLimits { queue_records, queue_bytes,
  max_record_bytes }`. Each sink gets its own `sync_channel` bounded by
  `queue_records`. Admission uses `try_send`, so a full queue drops the
  record with reason `queue_full` instead of blocking. The producer is
  never blocked, so a stalled sink cannot stall task control or input.
- **Oversize rejection.** A serialized record larger than
  `max_record_bytes` is refused at admission with reason
  `record_too_large` rather than queued or truncated, so the bound is
  real at the boundary.
- **Redaction before persistence.** `redact_record` runs once at
  capture, before any sink sees the record. No sink receives unredacted
  data, so a misconfigured sink cannot leak.
- **Fan-out with independent health.** `publish` returns
  `PublishOutcome { delivered, dropped, drop_reason }`. Per-sink state
  is exposed as `SinkHealth { name, queued_records, queued_bytes,
  capacity_records, capacity_bytes, dropped, last_error, degraded }`.
- **No recursive failure storm.** A `write_batch` failure marks that
  sink `degraded`, records `last_error` and counts the batch as
  `write_failed`. Failed records are never re-published into the hub,
  so a broken sink cannot amplify its own failure into the other sinks
  or the producer.
- **Shutdown is bounded.** `shutdown(deadline)` returns
  `ShutdownReport { drained_sinks, incomplete_flushes }` where
  `incomplete_flushes` carries `(sink name, records still queued)`.
  Exceeding the deadline reports the remainder rather than hanging or
  claiming success. Publishing after shutdown is refused with reason
  `shutdown_stopped`.

Drop reasons are stable lowercase strings: `queue_full`,
`record_too_large`, `serialize_failed`, `write_failed`,
`shutdown_stopped`.

## Redaction

`RedactionPolicy` has three fields and they are not interchangeable.

- `forbidden_key_substrings` — lowercase substrings marking a key as
  credential-bearing. The value becomes `[REDACTED]`.
- `known_safe_keys` — **exact** keys exempt from the substring rule.
  Matching is exact, never substring, so a key cannot borrow its way
  onto the list.
- `secret_values` — known secret strings scrubbed from value text
  regardless of key.

`known_safe_keys` exists because the pinned GenAI vocabulary legitimately
contains the substring `token`. Without the exemption the default policy
silently rewrites `gen_ai.token.modality` and every
`gen_ai.usage.*_tokens` attribute to `[REDACTED]`, erasing the token
accounting the contract requires while appearing to work. The exemption
list is derived from the core vocabulary constants so it cannot drift
from the conventions snapshot.

Redaction covers all three payload arms — log attributes, span attributes
and metric dimensions — because each carries its own map. A policy that
only scrubbed logs would leak through a span.

Redaction cannot prove that arbitrary prose contains no unknown secret.
It is one layered control, not a guarantee.

## Pinned GenAI instrumentation

`fluzo_core::telemetry::instruments` and `attributes` pin the names,
units and attribute keys from the OpenTelemetry GenAI semantic
conventions at revision `8ffdf568e1b4391a99adb081db16e8102e36918e`,
recorded as `GENAI_CONVENTIONS_REVISION` and asserted by the core test
`pinned_instrument_names_and_units_match_the_conventions_revision`.
Upgrading requires explicit review; revisions are never mixed.

Units are `"{token}"` (`TOKEN_UNIT`) for token counters and `"s"`
(`SECONDS_UNIT`) for durations. Instruments are the streaming
`gen_ai.client.inference.usage.*` family. `MetricSnapshot` carries
`unit`, `kind`, `aggregation`, the aggregation window and an explicit
`MetricTemporality`. Temporality is a field rather than an inherited SDK
default so a stored snapshot keeps meaning the same thing once the OTLP
increment lands.

Span names follow `"{operation} {serverRequestModel}"`, e.g.
`chat flash-halogen`, via `gen_ai_span_name` — one span per logical
provider operation. `attributes::ABSENT_BY_DEFAULT` lists
`gen_ai.input.messages`, `gen_ai.output.messages` and
`gen_ai.system_instructions` as never emitted by default.

## Token accounting

`UsageNormalizer` keeps a monotonic ledger and is deliberately
conservative.

- **Cumulative deltas.** Providers report cumulative usage per chunk.
  The ledger stores the last observed cumulative value per field and
  adds only the positive delta, so repeated identical cumulative reports
  add nothing and a growing report yields only the increment. A final
  usage-only chunk carrying the whole operation's usage is therefore
  absorbed correctly with no double count.
- **Subsets never join totals.** Cache-read, cache-write and reasoning
  counts are tracked but excluded from the input/output totals, since
  they are subsets of those counts. The pinned example: 100 in (40
  cached) + 20 out (5 reasoning) is 120 tokens, not 165.
- **Missing counts are omitted, not zeroed.** A field never reported is
  `None`; a field reported as zero is `Some(0)`. These are different
  facts.
- **Incomplete operations keep known usage.** `finish_incomplete()`
  returns the accumulated usage tagged `UsageSource::Incomplete`, and
  returns `None` when nothing was observed, so an empty operation is not
  reported as a zero-usage operation.
- **Retries are separate.** `report_retry` accumulates into retry
  consumption tagged `UsageSource::PhysicalRetry`, never into final
  response usage. `total_known()` reports both. A retry that consumed
  tokens without producing a final response is visible as consumption
  without output.
- **Metrics survive unsampled spans.** A metric record is its own
  record; `sampled: false` on a span does not suppress the
  corresponding metric.

## Local JSONL archive

`.fluzodrive/telemetry/telemetry-<n>.jsonl`, one JSON object per line,
fsynced per batch.

- The directory is created `0o700` and files `0o600`; the existing
  `.fluzodrive` containment rules apply. A symlinked archive root is
  refused.
- `with_rotation(rotate_bytes)` sets the per-segment bound. Exceeding
  it advances `segment_index` and starts a new segment. Rotation never
  rewrites or removes existing segments.
- **Total archive size is not yet bounded.** The bound is per segment;
  segment count grows until purge lands in DAT-02e (#76). Do not
  describe this archive as a ring or as capped.
- `read_archive` returns `ArchiveScan` over `ArchiveLine::Complete` and
  `ArchiveLine::Incomplete`. A torn trailing record is reported as
  incomplete rather than silently dropped or repaired, so a crash
  boundary stays visible in the archive itself.
- A record line containing a newline is rejected rather than escaped, so
  the one-object-per-line invariant cannot be broken by payload content.

## Explicitly not implemented

- **OTLP HTTP/protobuf export** — #88. The offline cache has
  `opentelemetry-otlp 0.13.0` but every HTTP client option
  (`http-proto`, `reqwest-client`, `reqwest-blocking-client`,
  `surf-client`) requires `opentelemetry-http`, which is not cached, and
  the crate's `metrics` feature pulls `grpc-tonic`. The accepted
  transport cannot be built offline, so the increment was deferred
  rather than substituted with gRPC.
- **`tracing` bridge** — the hub is sink-agnostic and an exporter plugs
  in as another sink, but no `tracing` subscriber is wired.
- **Total archive retention/purge** — DAT-02e (#76).
- **Compose stack and dashboards** — #14.

No end-to-end delivery, backend ingestion, dashboard correctness or
performance claim is made. Producer and serialization cost under
export-disabled, healthy and degraded sinks is unmeasured.

## Verification

```sh
cargo test -p fluzo-core --locked --offline telemetry
cargo test -p fluzo-runtime --locked --offline telemetry
python3 -B -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_dev_setup.py
```

Core covers the pinned vocabulary, the streaming-subset absence check,
cumulative delta arithmetic, subset exclusion, missing-vs-zero, retry
separation, temporality and span naming. Runtime covers queue-full and
oversize drop reasons, redaction before persistence, redaction across
all three payload arms, the default policy preserving pinned GenAI
attributes while still catching arbitrary credential-shaped keys,
degraded-sink isolation without a recursive storm, audit/dependent
atomicity, audit failure not disabling diagnostic sinks, bounded
shutdown with an expired deadline, and archive rotation with torn-record
reporting.
