# Selected implementation practices

Modified by Fluzo from the four rules recorded in [provenance](../ORIGIN.md). This local adaptation replaces upstream examples and outgoing reference chains; it is not the complete original handbook. No source retrieval is required to use it.

## Ownership

Prefer borrowing for local read-only work and slices or str when they express the needed contract. Owned values are appropriate when transferring responsibility or preserving an independent snapshot. Not every clone allocates; inspect the actual type and workload rather than labeling every clone a defect.

For public APIs, evaluate lifetimes, mutation visibility, compatibility, and serialization before changing owned values to references. When the consumer requires an owned versioned application port, borrowing across that port is not a valid optimization. Do not share mutable runtime state with widgets in violation of the approved architecture.

Use a straightforward loop for control-heavy work and iterator composition where it remains readable. Avoid materializing collections solely to discard them, but do not introduce complex lifetime or pointer machinery without a demonstrated benefit.

## Errors

Represent recoverable failures with Result and meaningful typed categories using the project's existing error strategy. Invalid configuration, network errors, and ordinary user input are not reasons to panic or silently fall back to defaults. Preserve root causes and safe diagnostic context. Distinguish denied, cancelled, failed, and unknown outcomes where the contract requires it.

Retryability depends on protocol, idempotency, deadline, and completion evidence. A transport failure does not prove that a mutation did not occur. Never replay an uncertain effect simply to convert an error into success. Fixture setup may use expect to make a broken prerequisite explicit; that does not justify panic in a recoverable production path.

## Bounded asynchronous work

Bound record count, payload size, workers, and in-flight tasks. A bounded queue with unlimited producers is not a bounded-memory system. Check oversized individual messages as well as aggregate retention.

Select admission behavior by path and consumer policy: backpressure with deadlines for ordinary work, bounded nonblocking loss accounting for optional diagnostics, and independent reachability for control/cancellation. Mandatory durable audit must not be replaced by successful channel delivery or dropped as if it were optional diagnostics.

Dropping a future or task does not establish remote termination. Track ownership, shutdown, joins, and uncertainty explicitly. Release reservations only under the actual resource/completion contract; do not let a timeout erase a still-running operation. Ensure input and cancellation can remain responsive when ordinary queues are full.

## Performance

Establish correctness, then measure a representative workload before specialized optimization. Record baseline, change, environment, and outcome, including memory or tail latency when relevant. An unmeasured suspected improvement is a hypothesis, not a regression finding or benchmark result.

Do not install tools or change compiler settings merely because generic guidance recommends them. Viewport bounds and cancellation responsiveness may already be product requirements; enforce those without disguising speculative micro-optimizations as architecture requirements. Preserve unsafe-code restrictions and compatibility constraints.
