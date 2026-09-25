---
name: fluzo-deterministic-testing
description: Use when designing or changing Fluzo tests, fixtures, simulators or CI checks; prevent live inference and false success.
user-invocable: true
---

# Deterministic verification

1. Read the owning issue and AGENTS.md test pyramid. Map the requirement to a
   named regression and expected failure before writing the implementation.
2. Use pure unit tests for rules. Use real disposable files/SQLite/processes for
   their boundaries. Use strict harness-owned HTTP listeners for provider adapters;
   HTTP simulator/native-loop acceptance is future work, not implemented here yet.
3. Isolate config, credentials, proxies, repository and ports. Only listeners
   registered to that test run are valid targets; arbitrary localhost is not safe.
   Never probe real endpoints or fall back to live models. No model credentials.
4. Drive races through barriers/acknowledgements, with injectable clocks for domain
   time and bounded real-time deadlines for OS/PTY work. Avoid arbitrary sleeps.
5. Include wrong input, denied operations, missing/unexpected requests, cancellation,
   partial effects, unknown outcomes and failed persistence where applicable.
6. Independently verify changes and unchanged tests. A provider success string or
   a simulator editing files does not establish native-agent correctness.
7. Run the targeted test followed by `cargo test --workspace --locked --offline`
   and `python3 -m unittest discover -s scripts -p 'test_*.py'`.
8. Report live inference as not_run unless explicitly authorized and executed.
   Missing future suites are unverified, not passed by bootstrap checks.

## Calibration cases

Reject a mock that edits the fixture itself, a timeout that releases an uncertain
slot, a 429 automatic retry, and a test reading the developer's .fluzo. Accept a
real tool modifying a disposable fixture whose assertions are independently rerun.
