# Selected Rust rules, adapted for Fluzo

Modified on 2026-09-24 from the four rules identified in ../ORIGIN.md.
Examples and outgoing reference chains were replaced with these self-contained
procedures; this is not an unmodified upstream distribution.

## Ownership

Prefer borrowing for local read-only work and slices/str over collection references.
Clone only when the ownership boundary or independent snapshot requires it.
Not every clone allocates: assess the actual type, not a generic cost claim.
Owned versioned application DTOs remain required; borrowing across that boundary
is not a valid optimization. Do not share mutable runtime state with widgets.

## Errors

Represent recoverable failures with Result and typed categories. Invalid config,
network failures and user input are not panic cases or permission to use silent
defaults. Preserve safe error context without credentials. Retryability requires
specific protocol/completion evidence; unknown side effects must not be replayed.
Tests may use expect to identify broken fixture setup, not to hide production errors.

## Bounded async work

Bound both record count and payload bytes. Bound workers and in-flight tasks too:
a bounded channel plus unlimited spawned producers is not bounded memory.
Select admission behavior by path: backpressure with deadlines for ordinary work,
nonblocking bounded loss accounting for diagnostic sinks, independent priority
for control. Mandatory audit is never dropped or replaced by channel delivery.
Dropping an awaiting task does not establish remote termination. Track ownership,
join/shutdown behavior and uncertainty explicitly rather than blindly releasing slots.

## Performance

Establish correctness, then measure a representative workload before specialized
optimization. Record baseline, change and outcome, including tail latency and memory.
Do not install profilers or change CPU flags, panic strategy, allocator or toolchain
merely because a general guide recommends them. UI viewport bounds and cancellation
responsiveness are architecture requirements, not optional premature optimizations.
