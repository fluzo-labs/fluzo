# Validation cases and consumer profile

## Maintenance cases

Use an independently copied skill folder and a disposable Rust project without external dependencies where practical. Inspect all fixture code before execution. Do not create a permanent test framework or install new libraries solely to validate these instructions.

| Case | Required observation |
| --- | --- |
| Positive regression | Fixture compiles; intended assertion fails before the scoped change and passes afterward |
| Protected tests | Test and build inputs remain byte-identical across the application change |
| Unexpected HTTP request | Strict simulator returns an error and the harness reports the mismatched step |
| Missing required request | Final verification rejects an unfinished script, even if the client reports success |
| Cancel then late result | Acknowledged cancellation followed by a released response cannot replace current state |
| Broken scheduling | The same controlled schedule detects a version that accepts the stale result |
| Resource separation | Parallel fixtures do not share configuration, ports, output paths, or mutable process environment |
| Foreign endpoint | Client or approved isolation rejects an endpoint not owned by the fixture; no fallback |
| Partial mutation and timeout | Outcome stays uncertain until independent reconciliation; no blind retry |
| Missing dependency cache | Offline command is reported blocked; no network, lockfile rewrite, or installation fallback |
| Review-only activation | Report only; no file writes or command mutations beyond authorized checks |

These are acceptance scenarios, not a claim that every case has already run. Record a limited positive/negative fixture exercise separately from an evaluation of agent behavior or a production runtime.

## Fluzo profile, not a universal policy

For a consumer that actually uses Fluzo's architecture and instructions, retain the existing test pyramid: pure rules, contracts, adapters, deterministic native execution when implemented, reference terminal acceptance, and separately authorized live evaluation. Preserve the rule that a simulator cannot edit the repair fixture itself.

The original profile rejects automatic 429 recovery, releasing uncertain execution slots merely on timeout, and reading the developer's application configuration. Apply these only after confirming the current consumer policy; other projects may define different safe retry and ownership semantics.

Discover commands from the consumer. The following commands belong to an existing compatible Cargo workspace and Python script suite, run from that consumer's root, not from this skill directory:

```bash
cargo test --workspace --locked --offline
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Run the Python command only if those scripts actually exist and have been reviewed. A lockfile or cached dependency failure is not a product-test failure. Do not invent native HTTP or agent-loop acceptance from bootstrap checks; missing future suites remain `not_run` or `blocked` according to the task.

## Distribution checks

Verify matching folder/frontmatter names, complete local references, license inclusion, and portability outside the collection. No skill downloads, sibling imports, fixed personal paths, or unapproved service dependencies are allowed. Preserve the included original MIT copyright notice when copying or adapting this folder.
