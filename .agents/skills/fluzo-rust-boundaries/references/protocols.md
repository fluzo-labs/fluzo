# State ownership and application contracts

Read when shared types, application ports, events, or component responsibilities change.

## Review the meaning of each crossing

For each public interface, identify the caller, owner, input validation point, effect owner, result, and lifecycle. Follow actual call sites rather than trusting module names or comments. A struct placed in a neutral crate can still expose an implementation dependency.

Keep protocol values separate from transport machinery where the consumer's architecture requires it. Examples of potential leaks include concrete database connections, provider clients, widget objects, channel endpoints, cancellation handles, callbacks, or shared mutable domain state. These are findings only relative to the approved contract; not every Rust API must be serializable or use owned values.

When an owned snapshot is intentional, preserve its isolation semantics. Do not replace it with a borrowed or shared reference solely to remove a clone without checking lifetimes, threading, mutation visibility, and serialization needs. Conversely, do not force allocation or version fields into private local APIs that do not need them.

Review typed error meaning, not just the use of an enum. Distinguish validation failure, denial, cancellation, transport failure, and unknown outcome where the consumer exposes them. Preserve diagnostic context without leaking secrets or adapter internals into a stable public contract.

## Async and versioned protocols

An acknowledgement may mean queued or accepted, not completed. Identify which event or query establishes completion, which operation ID correlates results, and how duplicate, late, or out-of-order events are handled. Do not release ownership or retry a mutation based solely on a transport timeout when the outcome is uncertain.

For snapshots and subscriptions, check cursor/gap detection, resynchronization, stale version rejection, and backward compatibility as required. Define ownership of cancellation and subscriber teardown; a UI may request cancellation without owning the runtime's internal cancellation token.

Trace effectful dependencies and standard-library use in allegedly pure modules. Reading environment variables, time, files, or spawning processes can violate a pure-core policy without adding any Cargo dependency. A graph checker cannot detect all such behavior. Separate semantic review findings from compiler and metadata evidence.

## Regression selection

Use tests that exercise the actual contract: owned values remaining stable after source mutation, acknowledgement not counted as completion, stale responses rejected, error mapping retaining meaning, or a forbidden implementation type no longer appearing in a public interface. Choose only cases relevant to the consumer.

Use compiler checks, existing API/architecture tests, and scoped source review as complementary evidence. Negative compile cases should fail for the intended API boundary, not for unrelated syntax or missing dependencies. Never replace a meaningful integration contract with a mock that bypasses the crossing being tested.
