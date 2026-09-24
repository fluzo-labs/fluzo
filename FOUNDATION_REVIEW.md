# Foundation self-review: FND-01 and FND-04

Date: 2026-09-24. Scope: the pending local bootstrap, not runtime acceptance.
This review does not approve a merge or close either issue. The initial review
made no code changes. A subsequent authorized correction resolved both findings
below; no commits, pushes or GitHub mutations were made.

## Authority

- [FND-01, #1](https://github.com/fluzo-labs/fluzo/issues/1): four crates, MIT metadata, lockfile, toolchain/MSRV and dependency boundaries.
- [FND-04, #4](https://github.com/fluzo-labs/fluzo/issues/4): formatting, lint/build/unit/boundary checks without private inference dependencies.
- [Architecture baseline](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/ARCHITECTURE.md): sections 3, 5.4 and 15; A01/A02/A08.
- [PRD baseline](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/PRD.md): sections 32, 33, 45.2, 51-53.

The source snapshot is uncommitted. A later PR must identify its exact revision
and rerun checks; this report is evidence for the reviewed working tree only.

## Original findings, now corrected locally

The reproductions and line references below describe the pre-fix snapshot. They
are retained as failure evidence, not as claims that the current code still fails.

### P1: inactive features bypass the bootstrap boundary policy

Location: `scripts/check_dev_setup.py:42-44`, invoked without selected features
at `scripts/check_dev_setup.py:84-88`.

The checker rejects activated features in resolved nodes, but does not reject
new feature declarations or optional dependencies that are inactive in the default
graph. This contradicts the documented bootstrap policy that new features need
review. It permits a future forbidden production path to be introduced while the
default CI check stays green.

Reproduced in a disposable copy using actual Cargo metadata, not only a synthetic
JSON graph. Append this to `crates/fluzo-tui/Cargo.toml` in the disposable copy:

```toml
[dependencies.fluzo-runtime]
path = "../fluzo-runtime"
optional = true
```

After regenerating the copy's lockfile offline, passing default metadata to
`validate_graph` succeeds. Resolving with
`--features fluzo-tui/fluzo-runtime` instead exposes the forbidden tui -> runtime
path and is rejected. The working repository was not changed by this experiment.

Correction: while bootstrap features are unsupported, reject declarations in
workspace package metadata as well as activated features. Once features are
introduced deliberately, define supported combinations and check each resolved
graph. Add a regression using a real optional dependency and an inactive feature;
the current activated-node mutation test does not cover this case.

### P2: LSP request deadlines can be extended indefinitely by notifications

Location: `scripts/check_lsp.py:53-65`.

Once the deadline expires, `max(0.01, deadline - time.monotonic())` still permits
another queue read. A stream of progress/diagnostic notifications keeps the loop
running past its deadline and appending notifications. It can return a successful
reply after expiration instead of failing. Since outer index/diagnostic deadlines
are checked only after request returns, they do not bound this case either.

Reproduced without a real server using the actual Client.request method: replace
the message queue with three progress notifications followed by response id 1;
mock monotonic values as 0, 61, 62, 63, 64. The 60-second request returns
`accepted after deadline` and stores three notifications instead of timing out.

Correction: explicitly test remaining time before every receive and raise a
TimeoutError once it is nonpositive, even if messages are immediately available.
Propagate outer deadlines rather than resetting them per request. Add deterministic
unit tests for notification flooding, late responses and server EOF. The fixture's
Cargo subprocess at lines 157-158 also lacks a timeout; bound that subprocess and
retain useful failure diagnostics so CI failure paths remain inspectable.

## Acceptance assessment

| Criterion | Observed evidence | Status |
| --- | --- | --- |
| Four-crate topology | core independent; runtime/tui -> core; cli composes both | Current default graph passes |
| MIT, lockfile, toolchain/MSRV | Workspace MIT, lockfile present, Rust 1.98.0 and MSRV 1.98, edition 2024/resolver 3 | Locally verified; lockfile still untracked |
| Forbidden dependency detection | Current graph, illegal edges and real inactive Cargo feature fixtures pass | Original gap corrected locally |
| Format, lint, build and unit checks | All documented checks executed successfully | Locally verified |
| No private inference requirement | Dependency-free Rust source, no provider path; isolated source export passes with empty HOME/CARGO_HOME | Locally verified for bootstrap only |
| LSP fixture | Definitions, references, rename, E0308, deadline, EOF and subprocess regressions pass | Original gap corrected locally |
| Clean checkout / fork CI | Exported pending source tested without prior target or local Cargo cache | Committed checkout and remote fork workflow not verified |
| Published evidence | This local report | Not linked from a completing PR/issue yet |

Five Rust tests and seventeen Python tests pass after correction (the original
review ran seven Python tests). No additional Rust implementation
correctness issue was identified in this small help/version-only slice. These
results do not prove future permissions, confinement, provider or TUI behavior.

## Executed checks

All commands below passed both in the working tree and in a disposable export of
the tracked plus nonignored untracked pending source files:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_lsp.py
cargo build --workspace --locked --offline
```

The isolated run used empty HOME/CARGO_HOME, no inherited credentials or proxies,
and no copied target directory. It retained PATH and the prepared RUSTUP_HOME;
CARGO_NET_OFFLINE was true. Each command had a 180-second outer safety timeout in
the review harness. This is not a committed clean checkout, an OS-level egress
restriction, or evidence of toolchain downloads working on a remote runner.

Crush references found nine occurrences of RuntimeAvailability across three crates.
Local source and workflow review found no model calls or embedded credentials in
the bootstrap. Git diff --check passed. Live inference was not_run. No remote CI,
release, packaging, visual acceptance or model-quality result is claimed.

## Correction evidence

- The graph checker now rejects workspace feature declarations and optional
  dependencies even when absent from activated resolve nodes. Real Cargo fixtures
  cover an empty inactive feature, an implicit optional-dependency feature and an
  explicit dep: feature. All three failed before the fix and now pass by rejecting
  the unsupported configuration.
- LSP requests check expiration before dispatch, before queue receives and after
  receipt. Outer indexing/diagnostic deadlines are propagated to requests. Queue
  exhaustion becomes TimeoutError; EOF and truncated messages wake the waiting
  receiver with EOFError rather than silently ending the reader.
- The renamed-workspace Cargo test has a 120-second timeout; rustup discovery has
  a 30-second timeout. On fixture failure, bounded tails of subprocess output and
  server logs are emitted before temporary files are removed.
- Regression tests cover notification flooding, late replies, inherited/expired
  deadlines, empty queues, EOF/truncated payloads, normal replies, server errors
  and Cargo timeout/failure propagation. The first pre-fix EOF test run was stopped
  after exceeding 60 seconds; its assertion was tightened to prevent real waiting
  before rerunning and observing the expected failures. No failed attempt is
  counted as a passing run.
- All eight commands listed above passed again in both the working tree and an
  isolated pending-source export with empty HOME/CARGO_HOME and no prior target.
  The real rust-analyzer fixture also passed; no live inference was used.

## Next delivery increment

Both reviewed defects are corrected locally. Keep FND-01/FND-04 open until a
reviewed PR records its exact revision and applicable CI evidence. Configuration
(#2) and the application port (#3) follow acceptance of the foundation; publication
remains subject to explicit authorization.
