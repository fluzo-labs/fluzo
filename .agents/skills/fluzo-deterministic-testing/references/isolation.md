# Isolation and controlled scheduling

Read before constructing a fixture or diagnosing nondeterministic tests.

## Isolate the resource, not just its name

Give each test a private temporary root, repository when needed, configuration directory, and output paths. Use exclusive creation and safe lifetime ownership. Do not reset, clean, or delete a consumer checkout to make a test reproducible. Never traverse unreviewed symlinks during fixture setup or cleanup.

Build a minimal child-process environment rather than inheriting arbitrary credentials, proxies, runtime endpoints, Git helpers, and user configuration. Set HOME, relevant platform/XDG directories, and application config explicitly. Preserve only reviewed toolchain/cache paths needed to run offline. Inspect inherited ancestor Cargo configuration and executable overrides as well. A different HOME alone does not isolate every discovery mechanism.

For parallel tests, avoid process-wide environment or current-directory changes. Pass configuration as values or spawn isolated child processes with their own environment and cwd. Shared caches are read-only inputs only when their content and potential executables have been reviewed; do not advertise a writable shared cache as isolation.

Use real temporary files, databases, and child processes where the requirement is their actual behavior. Test durable state from a separately opened connection or process when durability matters. A mocked successful write does not verify persistence, permissions, transactions, or crash recovery.

## Own listeners and forbid unintended traffic

Bind a test-owned listener directly to a loopback address and an ephemeral port; keep it open while deriving the endpoint. Never reserve a port by finding a free one and reopening it later. Register exact scheme, address, port, and run identity with the fixture before the client starts. Do not treat arbitrary localhost endpoints as safe.

Prevent redirects, proxy routing, DNS fallback, alternate providers, and background telemetry from escaping the approved fixture destinations. Inject a transport or resolver that rejects unregistered destinations where possible. A listener counter proves only what that listener received, not that the process made no other connections. If asserting absence of all egress, use an authorized network-isolation mechanism or comprehensive transport instrumentation and describe its coverage; do not silently change host firewall rules.

`CARGO_NET_OFFLINE` and Cargo's `--offline` constrain Cargo dependency access, not application sockets, build scripts, or subprocesses. No fixture may rely on model credentials or a real provider being available.

## Three separate sources of variation

| Dimension | Control | Remaining limitation |
| --- | --- | --- |
| Generated inputs | Recorded seed, explicit IDs, stable fixtures | A seed alone does not control scheduling |
| Domain time | Injectable clock and explicit advancement | Does not interrupt a blocked OS call |
| Scheduling and effects | Channels, barriers, acknowledgements, controlled executor | Selected schedules are not exhaustive |

Wait for a meaningful readiness event rather than sleeping and hoping work has started. Keep bounded wall-clock deadlines around channels, process joins, sockets, PTYs, and shutdown. Avoid resetting an outer deadline on every retry. Polling is appropriate only for an inherently asynchronous external observation, with a finite deadline and useful timeout evidence.

A cancelled request needs an identity or generation fence so a late result cannot overwrite newer state. Cancellation is not proof that an external mutation did not happen. Preserve uncertain outcomes and reconcile them before retrying or releasing ownership under the consumer's policy.

## Cleanup and failure evidence

Stop workers, close listeners, terminate only children owned by the fixture, and reap them with finite deadlines. Cleanup must run on failed assertions and partial setup too. Preserve the original error as well as cleanup errors. A deadline terminating a hung fixture is a failure, not a successful cancellation test.

Retain only approved diagnostic artifacts, with bounded size and synthetic/redacted data. Do not keep real environment dumps, private transcripts, or credentials. Record enough context to distinguish a product failure from a broken harness without masking either.
