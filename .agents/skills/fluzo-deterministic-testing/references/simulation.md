# Strict simulations and independent evidence

Read for HTTP/provider adapters, streaming protocols, retries, or agent repair scenarios.

## Specify the protocol before running it

Represent each expected request with method, path, required headers or body fields, permitted cardinality, and ordering constraints. Use a partial order for intentionally concurrent requests rather than inventing a total order. Match stable semantic values rather than volatile timestamps or incidental serialization order.

The harness must fail on both unexpected requests and unconsumed required steps. A catch-all success response hides regressions. Bound request sizes, connections, response chunks, and total duration. Close or report incomplete streams according to the scenario rather than letting a stalled client hang the suite.

Exercise malformed input, missing fields, wrong status, partial streams, cancellation, connection loss, duplicate responses, and persistence failure where relevant. Make retry behavior explicit: allowed attempts, eligible failures, backoff source, deadline, and idempotency requirement. A 429 or timeout is not universal permission to retry a mutation. Conversely, do not forbid a documented safe retry merely because a different product disallows it.

Check negative destinations with a separate fixture-owned observer when needed, for example a redirect target that must receive zero requests. Combine this with a transport allowlist or permitted network isolation before claiming that no non-fixture traffic occurred. Report weaker evidence as weaker evidence.

## The system under test owns effects

A provider simulator may emit a proposed edit or tool request. Only the actual consumer loop and executor may authorize and perform the operation being verified. If the simulator edits the file directly, the test proves the simulator, not native execution.

For a repair scenario, establish a fixture that compiles but fails a specific assertion. Record protected test and build-config content before execution. The authorized system modifies only the intended source. Then an independent harness reruns the original tests, checks unchanged protected inputs, and inspects resulting files and durable records. Do not accept a success string, exit code alone, or a modified test as proof of repair.

Separate:

- Protocol simulation: requests and responses conform to the script.
- Component integration: real adapters and resources behave as expected.
- Native end-to-end execution: the actual application loop drives tools and persistence.
- Agent evaluation: an identified model and agent follow the procedure under controlled conditions.

Passing one layer does not imply the others. Do not rename a miniature fixture into a native E2E test.

## Evidence record

For each case, record the requirement, source/fixture identity, command, cwd, toolchain, feature and target selection, seed or schedule where applicable, expected failure, actual outcome, and cleanup result. Preserve failed attempts and distinguish `passed`, `failed`, `blocked`, and `not_run`.

If the work has no commit yet, identify the exact tested content with hashes and mark it uncommitted. Do not attribute checks to a later commit without establishing content equivalence. Live evaluation is a separate explicitly authorized mode with bounded budgets; never silently downgrade a failed live run to simulation or promote a simulator result to live evidence.
