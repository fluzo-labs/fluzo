# HTTP scenario harness

SIM-01 ([#5](https://github.com/fluzo-labs/fluzo/issues/5)) implements test support
for PRD 45.1/45.2 and architecture A08, sections 15.3/15.4, at design baseline
`60c5b0732fb710cdf705476cee8d9156a5ecd971`. This is protocol simulation, not native
agent acceptance, a provider implementation, or a production HTTP transport.

## Ownership and dependencies

`crates/fluzo-runtime/tests/support/mod.rs` is compiled only by integration tests.
`http_simulator.rs` holds version-1 synthetic scenarios and independent assertions.
No crate, runtime API, feature flag, background service or production dependency
is added. The simulator never executes tools or edits application fixture files.

Hyper 1.11.1 handles HTTP/1 parsing and framing; hyper-util 0.1.20 provides Tokio
I/O, http-body-util 0.1.5 bounds bodies, Tokio 1.53.1 supplies current-thread
scheduling, and serde_json 1.0.151 compares request JSON semantically. Hyper was
chosen over the initial Axum candidate because this small harness needs direct
body-frame faults and connection ownership, not routing or middleware. All are
pinned dev dependencies resolved from the existing local cache. Cargo.lock pins
transitives. No dependency download was needed for implementation.

The graph checker reviews their versions and feature ceilings, keeps them out of
normal/build paths and restricts direct test imports to runtime. HTTP/2, TLS,
proxy discovery, general URL connectors and multi-thread Tokio are not enabled.
Tokio macros expand the existing procedural-macro feature ceiling during tests;
the normal/build graph remains the reviewed Serde/TOML graph. Conditional Windows
and WASI packages are locked and checked, not supported-platform test evidence.

## Scenario contract

Each scenario supplies version 1 and ordered required steps. A step matches the
method, complete path/query, required headers and exact semantic JSON value
(object key ordering is irrelevant). Empty-body requests are explicit. Unknown
fields, missing fields, wrong values, unknown routes and duplicate calls fail;
there is no wildcard success or optional-step inference. Scripts must explicitly
list subsequent tool-result turns. Dynamic values are fixed synthetic values in
this slice, not guessed from production serialization.

Responses contain a status, headers and a bounded sequence of data frames,
one-shot gates or a deliberate disconnect. Gates let tests acknowledge a received
prefix before releasing the rest, without timing sleeps. Transport frames need
not coincide with SSE events or socket reads. The SSE fixture splits framing and
tool arguments, then supplies a finish reason, usage-only chunk and `[DONE]`.
Raw malformed payloads and invalid decision shapes are deliberately not repaired.
The two System One route examples validate synthetic JSON, not a finalized Laya
wire schema or real adapter validation.

Each fixture owns its ephemeral IPv4 loopback listener, state and exclusively
created temporary home/config/data/workspace directories. Its connector accepts
only that live fixture's exact endpoint, with no DNS, proxies, redirect following,
retry or fallback. A separate registered redirect observer must receive zero
requests. The command builder clears inherited environment and closes stdin;
callers must still own and bound any processes they choose to launch.

Limits are 32 steps, 64 KiB aggregate scripted body/header data, 64 KiB per received
body, 128 response actions per step, 16 headers per direction, 1024-byte paths,
16-byte methods, eight simultaneous connections and 64 admitted connections.
Hyper's request buffer is capped at 16 KiB. A fixture has a five-second monotonic
safety lifetime; operations and shutdown also have bounded waits. These are test
safety limits, not application settings or performance targets.

Call `finish().await` after client effects settle and assert the report. It stops
the listener, aborts/drains remaining connections and rejects missing steps or
incomplete bodies. Expected peer cancellation is counted only before harness
shutdown; shutdown itself cannot prove client cancellation. Drop aborts owned
workers and removes the private root on assertion failures. Reports retain bounded
typed failures and counts, never request bodies, credentials or untrusted paths.
A completed body is a server-side observation, not proof of remote application
completion. Deliberate disconnect is a script outcome, not inference success.

## Commands and isolation

From the repository root, after the separately authorized dependency preparation
`cargo fetch --locked` on a fresh machine:

```sh
cargo test -p fluzo-runtime --test http_simulator --locked --offline
python3 scripts/test_http_simulator.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_dev_setup.py
```

The first command tests protocol mechanics with fixture-restricted connectors.
Cargo offline mode does not isolate application sockets. The Python profile
builds the exact test executable offline, then runs it with a minimal environment
inside a new Linux user/network namespace using util-linux `unshare`. Only that
namespace's loopback interface is enabled, using Python's Linux socket ioctl;
there is no host interface, firewall or privilege configuration change. The test
process cannot reach developer loopback services or external inference. Existing
host configuration, proxies and provider credentials are not passed to it.

Linux user/network namespaces, Python 3.11+ and `unshare` must be available. A
kernel/container/AppArmor restriction is a failing prerequisite, not permission
to fall back to host networking. The profile has 120-second compilation,
60-second namespace and 45-second executable safety limits. Development CI runs
this profile after dependency preparation. Remote runner compatibility remains
unverified until that workflow actually runs.

## Evidence and exclusions

The starting tree at `0b54a95` matches merged FND-03 tree
`3068e3a3c596aab1342959a9b8a818db5484b6a3`; implementation stayed on the existing
branch and preserved the index. The authorized pending skill migration remains
part of this delivery, including collection inventories, licenses and validators.

Focused local evidence: 12 HTTP tests pass, including ordered multi-turn JSON,
missing/unexpected/duplicate traffic, malformed HTTP and oversized bodies,
fragmented SSE, both System One routes, status/malformed-answer passthrough,
redirect target zero requests, explicit retry rejection after 429, partial
cancellation/disconnect, independent parallel roots and shutdown-not-cancellation.
The private-network profile passes the same 12 tests. Thirty Python tests pass,
including poisoned-environment and fail-closed namespace checks, forbidden
production/test dependency edges, and skill provenance/checksum regressions.

This was a new capability, so no pre-change behavioral regression was observed.
The pre-change 47 Rust tests passed. An auxiliary metadata-formatting command
initially had a Python syntax error; corrected graph inspection succeeded without
changing dependencies. Early intermediate checks exposed unused synchronization
helpers, resolved when the peer-disconnection acknowledgement was wired in.
No failed inference attempt or native repair is hidden by these harness results.

Final checks on the uncommitted working tree, with Rust 1.98.0 on Linux, passed:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
cargo build --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/test_http_simulator.py
python3 scripts/check_lsp.py
git diff --check
```

The workspace result is 59 Rust tests, plus 30 Python tests. The separate LSP
fixture passed definition, cross-crate references/rename, renamed workspace tests
and E0308 diagnostics. Self-review checked test-only dependency ownership, bounded
state/body/connection handling, cancellation evidence, isolation and preservation
of pre-existing skill changes. These are local results, not a remote CI run,
maintainer acceptance or issue closure.

Deterministic native repair, policy/tool/storage integration, typed Laya adapter
validation and telemetry ingestion remain later issues. Observability Compose,
optional live-model execution, packaging, performance and visual acceptance have
no implemented commands in this slice. Live inference is `not_run`, never passed.
The fixture connector is not a sandbox for arbitrary code; the isolated Linux
profile supplies the stronger no-external-network evidence.
