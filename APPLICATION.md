# Application protocol foundation

Library scope: [FND-03 (#3)](https://github.com/fluzo-labs/fluzo/issues/3),
[PRD sections 17, 23 and 51](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/PRD.md),
and [architecture A02/A07, sections 5.4 and 12](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/ARCHITECTURE.md).
This delivers an application protocol and deterministic scenario adapter, not an
agent engine, durable journal, permission service or production async transport.

## Contract and ownership

`fluzo_core::application::ApplicationPort` separates command, query, subscription,
poll and detach operations. Requests and results own serializable values; no
channels, cancellation handles, callbacks, database connections or widgets cross
the boundary. The trait itself is an in-process interface, not a wire payload.
Protocol version 1 is checked by the adapter before interpreting requests.

Task, session, workspace, attempt, operation, request and subscription identifiers
are distinct Rust types. Task states follow the PRD; attempt interruption and
unknown operation outcomes remain separate. Snapshots include state versions,
machine-readable wait reasons, verification state and elapsed active time.
Completion validation rejects missing verification and unresolved operations.
Fixture labels and supplied titles are untrusted content, not terminal-safe text.
The CLI demo prints only its static notice and typed numeric/state fields.

Queries take an immutable adapter reference and cannot submit commands through
that API. `fluzo_tui::inspection::inspect_task` consumes only the core port; its
pure fixture rejects every mutation/lifecycle call. The CLI supplies the concrete
scenario adapter. No production runtime dependency is added to TUI.

## Configuration port (UI-03 S1)

UI-03 S1 ([#44](https://github.com/fluzo-labs/fluzo/issues/44)) introduces
`fluzo_core::configuration::ConfigurationPort` alongside the task port, without
changing task protocol version 1 or inventing tasks for configuration edits.
Its separately versioned requests own typed values and a configuration request
ID. Configuration protocol 3 retains protocol 2's typed diagnostic codes and optional UTF-8
byte ranges in Invalid outcomes, including syntax, missing-version, size and
encoding failures. It rejects protocols 1 and 2 instead of silently discarding new
error fields; task protocol 1 is unchanged. Codes and ranges contain no source
values. Runtime's existing ConfigErrorCode import re-exports the core type.
Versions contain a service instance and revision; an old instance/revision
cannot authorize an edit, save, cancel or apply on a new snapshot.

The runtime `ConfigurationService::start` receives host-owned workspace/config
selection, typed CLI overrides and an explicit filesystem write policy. It
spawns one worker; only that worker parses files, validates candidates, writes
configuration and builds projections. The client methods use bounded nonblocking
queues and never wait for disk. An initial snapshot may return Busy until the
worker publishes discovery. There is no view-driven setup, inference or task
execution. S2 wires the setup host; S3 and C3 remain responsible for their
normal-view and Developer Menu composition/UI wiring.

Requests support Reload, Edit, Cancel, selected Save and selected Apply.
S2 (#45 R2) adds PrepareSetup with expected version and typed edits, followed by
ConfirmSetup with the prepared version. Preparation carries a bounded private
candidate and observed original on the worker, not in a widget. Snapshots expose
only a redacted SetupPreview and any retained backup basename. Confirm consumes
preparation; intervening actions invalidate it. Replayed request IDs still return
the retained outcome. Config protocol 3 is required; task protocol 1 is unchanged.
See CONFIGURATION.md for CreateOnly versus coordinated replacement and backup
failure/uncertainty semantics. No new dependencies or transport handles cross core. Submit
returns accepted identity, not effect completion. Status returns Unknown,
Accepted, Completed with version/outcome, or Failed with a typed category.
Identical request-ID retries return the existing record; changed payload reuse
fails. A post-save lost acknowledgement is reconciled by status without another
write. Records are not evicted to permit duplicate execution. Failed workers
turn unresolved accepted requests into Uncertain, not success or automatic retry.

Limits per service: 8 queued requests, 64 retained request records, 256 edits or
selected keys per request, 1 MiB budget for retained edit text/collection payloads,
and 1 MiB per configuration document. Key lengths and collection names are
bounded. Result queue capacity is 9; a slow client backpressures the worker rather
than creating more workers. Record exhaustion rejects new work. The host must
reconcile before replacing a service; IDs are not durable across crashes.
Instances are unique within the host process, not a cross-process wire identity.

`quiesce` closes admission and lets accepted work drain; status/snapshot calls
continue collecting results. Dropping the client does not prove an in-flight
filesystem call stopped and does not roll back a write. The host owns this
lifecycle, not a TUI consumer. Local filesystem calls can block the worker;
no OS-level hard deadline or forced cancellation guarantee is claimed.

Snapshots distinguish saved/draft/effective values, each origin and CLI locks,
and include effective presentation settings and pending restart keys. Sensitive
values and credential references are redacted. Public string projections remove
terminal control and bidi-format characters; these display-only values must not
be written back as replacement configuration. Edits contain only intentional
changes; runtime retains the original private document. The projection is not a
promise of general secret detection in arbitrary public user-entered strings.

Up to 64 versioned change records retain request identity and outcome, never
payload values. A saturating dropped count identifies older record loss. These
are bounded diagnostic projections, not durable audit or production sink
acknowledgements. No per-frame records are generated.

See CONFIGURATION.md for exact file/path/concurrency support and the distinction
between real persistence/presentation application and unavailable operational
application. Protocol round trips, bounded queues, accepted/completed separation,
duplicate requests, stale versions and actual adapter effects are tested in
`cargo test -p fluzo-runtime --lib --locked --offline configuration`.

## Acknowledgement, replay and cursors

A command acknowledgement means accepted, never completed. The scenario host must
explicitly call `advance()` to apply the scripted result. Queries and polls do not
advance it. Request status distinguishes unknown, accepted, completed and failed.
A deliberately lost acknowledgement can be reconciled by request ID; identical
request retries return the original acknowledgement without another dispatch.
Reusing an ID for a different command fails. Accepted request records are never
evicted to make room for duplicates; reaching the fixture limit fails closed.
These guarantees are in-memory and scoped to one driver, not durable exactly-once
execution after a crash or reconnect to a different host.

A snapshot and its cursor are captured together. Subscribing after that cursor
includes any retained intervening updates. Cursors carry an explicit host epoch;
old epochs, future cursors and gaps require resynchronization. Polls return a
bounded batch and advance that subscription only through returned updates.
Snapshots own their data, so later adapter mutations cannot change earlier views.
Task pages carry the snapshot revision; a mutation invalidates continuation rather
than silently mixing pages from different revisions. A full list spanning pages
requires restarting pagination if the revision changes.

## Deterministic scenario adapter

`fluzo_runtime::scenario::ScenarioDriver` owns an explicitly supplied initial
snapshot set and ordered command/result script. Unexpected or missing commands
fail `verify_complete()`. Scripted outcomes are fixtures, not simulated evidence
of real tool, provider, permission or verification execution. The fixture supplies
IDs, epochs and elapsed durations; there are no clocks, sleeps, environment reads,
network calls, file operations or background workers in the adapter.

Limits bound tasks, script steps, request records, subscriptions and retained
updates. Each configurable count is 1-4096; defaults are 64 tasks, 128 steps,
128 requests, 8 subscriptions and 64 retained updates. Titles/prompts are limited
to 8192 UTF-8 bytes and pages/batches to 64 entries. Fixed-size fields and bounded
text/counts give finite retained memory; these are fixture limits, not new user
runtime settings. A slow subscriber loses retained updates explicitly without
blocking host advancement or an otherwise admissible cancellation command.

The driver admits one pending scripted command per task. Another command for that
task receives a capacity error until the host advances the previous step. It does
not implement the production priority control queue or interruption of an actual
in-flight operation. A task can remain Running/Stopping/Unknown in a projection
while a later explicit cancel step is accepted. Stale task/approval versions and
unconfirmed force-stop requests are rejected before script consumption. No grant,
OS signal, rollback or process ownership is inferred from a scripted response.

Quiesce and shutdown belong to the host API, not the client port. Detaching a
subscriber does not stop a host or task. Quiesce rejects new start/resume/approval
commands while permitting scripted cancel/force-stop controls. Shutdown is
immediate and idempotent for this in-memory fixture: it retains task/request
projections, reports unresolved work and prevents later advancement. It neither
claims termination nor rewrites unknown outcomes as cancelled. Production bounded
shutdown, audit persistence and reconciliation remain later runtime work.

The port currently uses immediate synchronous calls for bounded in-memory work.
It does not permit blocking provider/DB work in an eventual input path: a future
embedded adapter must implement bounded messaging and independently owned host
execution, while preserving this request/result contract. No Tokio, RPC, socket,
HTTP simulator, second agent loop or extra crate is introduced by FND-03.

## Runnable demo and verification

```sh
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo
cargo test -p fluzo-core --locked --offline
cargo test -p fluzo-runtime --locked --offline
cargo test -p fluzo-tui --locked --offline
cargo test -p fluzo-cli --locked --offline
```

`fluzo demo` is a static protocol demonstration with a visibly synthetic pending
task. It does not open a TUI, load `.fluzo`, infer a provider or execute work. The
integration fixture uses an empty child environment and isolated HOME/workspace,
with an invalid config and unchanged source sentinel. It independently checks
that no files were created or changed and bounds the child with a real deadline.
The existing help/version and unavailable execution behavior are preserved.

Contract tests cover inspection/reconnection, acknowledgement versus completion,
lost replies and duplicate/conflicting IDs, bounded pagination, snapshot handoff,
cursor epochs/gaps, slow subscribers, stale approvals, force-stop confirmation,
structured failures, host lifetime, preserved uncertainty and fixture bounds.
Serializable DTOs round-trip using the existing TOML/Serde stack as a test codec,
not as a newly selected public wire format. No new dependencies or feature policy
changes are needed. TUI tests use core-only fixtures; runtime tests require no
terminal. These are protocol/component tests, not native agent E2E, visual review
or performance acceptance.

On 2026-09-25 the uncommitted FND-03 working tree passed the complete documented
foundation checks: format, workspace check, Clippy with warnings denied, 47 Rust
tests, build, dependency/skill validation, 24 Python tests and the isolated real
LSP fixture. The Python count includes two pre-existing skill-migration tests;
this phase does not modify or publish that migration. The CLI demo passed with
no application configuration or inference. LSP reported no diagnostics.

Review tightened fixture identity/consumption checks before acknowledgement and
made shutdown irreversible through quiesce, with targeted regressions. New APIs
had no executable before-change test; no failing baseline assertion is claimed.
No measured performance, real process termination, live inference or remote CI
result is implied. Live inference: `not_run`.

Acceptance/publication remains in GitHub. No automatic issue closure, commit,
push or PR is implied by these local implementation results.
