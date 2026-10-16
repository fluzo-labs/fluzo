# Agent guide

## Status and authority

Fluzo is a local-first coding-agent runtime in Rust at the development-bootstrap
stage. The four-crate workspace, bootstrap help/version binary, pinned toolchain,
local skills, development checks and the FND-02 typed configuration library exist.
See [CONFIGURATION.md](CONFIGURATION.md) for settings APIs and limits. FND-03 adds
an owned application protocol and an in-memory scenario driver; see
[APPLICATION.md](APPLICATION.md). DAT-01 adds the protected `.fluzodrive`
repository store with a WAL/FULL/foreign-key-verified SQLite journal and
atomic state/event commits; see [STORAGE.md](STORAGE.md). DAT-02a adds the
store-owned artifact layer under `.fluzodrive/artifacts/`: bounded capture
with truncation/redaction markers, finalization strictly before the atomic
reference+state+event commit, and verified crash/orphan/corruption
semantics; see [STORAGE.md](STORAGE.md). DAT-02d adds `fluzo history
list` and `fluzo history purge`: a read-only listing built from durable
records, a dry-run that provably changes nothing, confirmation that no
bypass flag can skip, shared artifacts preserved by re-owning them onto a
surviving holder, attachment-blocked sessions, and idempotent retry of an
interrupted purge; see [STORAGE.md](STORAGE.md). The telemetry archive is
not purged here (DAT-02e #76) and no forensic-erasure claim is made.
SIM-01 adds test-only strict HTTP/SSE fixtures
and a Linux private-network profile; see [HTTP_SIMULATOR.md](HTTP_SIMULATOR.md).
OBS-01 adds the diagnostics contract's two implemented paths: the SQLite audit
record committed atomically with its dependent state update, and a separate
bounded rotating JSONL archive under `.fluzodrive/telemetry/`, behind one
capture boundary with redaction, bounded queue, drop accounting and degraded-sink
reporting. Pinned GenAI semantic conventions, token accounting and the
retry/final split are typed in core. OTLP HTTP/protobuf export is deferred to
#88 because the offline cache cannot supply an HTTP exporter; see
[OBSERVABILITY.md](OBSERVABILITY.md). RUN-01 adds workspace identity, the
cross-process writer lock at `.fluzodrive/writer.lock` and the native file
confinement jail; see [WORKSPACE.md](WORKSPACE.md). Ownership is a held
`flock` descriptor, not a database row, because `BEGIN IMMEDIATE` releases
at commit and an approval wait spans the gap. Stale owners are reconciled
from the lock result, never inferred from PID liveness. The jail opens every
component with `O_NOFOLLOW` relative to a held descriptor and refuses
traversal, protected names, symlinks in any position, non-regular inodes
and hard-linked targets. It is not an OS sandbox and excludes only
cooperating processes. User-change protection is deferred to #97 because it
needs a Git working-tree baseline that does not exist yet. RUN-02 adds the
permission decision layer; see [POLICY.md](POLICY.md). Repository-origin
values may narrow a permission and may never widen one — a rejected
broadening is recorded rather than dropped. Grant matching is exact
equality on subject, workspace and backend, with no prefix or subset
matching anywhere. One-time consumption is a state transition performed
before dispatch, and an unconfirmed outcome keeps the grant spent. Session
YOLO converts only `AskUser`, is attributed to the mode in audit, has no
settings key, and is never restored by resume. Headless hosts declare
their capability up front, so an approval need returns a structured
outcome instead of waiting; `ApprovalUnavailable` is distinct from
`Denied`. RUN-04 adds session recovery and the private task-control
channel; see [RECOVERY.md](RECOVERY.md). A read-only open is enforced
by the driver, not by convention: `Store::open_read_only` cannot
create, migrate or write, and reading a record is never treated as
evidence of liveness — a running record reads as `Interrupted` and a
dispatched record with no confirmed result reads as `Unknown`. Reconcile
is read-only and apply is the only writer. Budget carries forward from
every prior attempt rather than resetting, and an unconfirmed
contribution marks the whole carry-forward uncertain. The control
channel is a private Unix socket bound only by the writer-lock holder,
verified per request against the live owner record, with a closed
request set that has no YOLO, lock-acquisition, resume or dispatch
variant. An inspection client cannot take the writer lock.
UI-01 adds an explicit interactive demo shell, composer and palette; see
[TUI.md](TUI.md). UI-02 part 1 adds themes and reversible session previews.
The current visual iteration follows Crush-style responsive conversation layout,
wrapped messages, expandable synthetic tool output and a growing borderless
composer. The original FLUZO relief wordmark is restored in the wide sidebar;
compact mode retains its one-line header. The original 80%-width top loader is
also restored, honoring work/idle, FPS 0 and reduced motion without changing layout.
The inspected Crush revision uses FSL-1.1-MIT; do not copy its source or artwork
into this MIT workspace by assuming an unrestricted license. UI-03 S1 adds a
separate typed configuration port and worker-owned local save/apply service;
see CONFIGURATION.md for its explicit coordinated-writer filesystem contract.
UI-03 S2 adds offline welcome/setup and headless configuration discovery using
configuration protocol 3. Under #45 R2 and fluzo-docs revision
`ed08afab9e2e3ac22a9ef5ef89e32911fcda1850`, ordinary CLI hosts may exclusively
create missing files. The accepted amendment in fluzo-docs revision
`3c7fc8e1f6934922ad66fe3eda8123854620839b` (PRD 26.1, architecture 10.2)
lifts the read-only-for-replacement boundary here: configuration is local, the
CLI runs `LocalWorkspace`, Fluzo replaces `.fluzo` on Save and trusts Git
instead of a backup copy. Outside edits are polled and surfaced as a banner
offering reload or keep-ours. UI-03 S3 (#46 R2) adds the normal
configuration-only shell and typed
basic/advanced editing with protocol 6, owned descriptors, operation
availability and bounded request reconciliation. The split is a registry-owned
`SettingTier` carried by every descriptor, so the editor filters by shared
metadata rather than hardcoding which keys are ordinary. Presentation Apply is separate.
S3 human-review revisions replace the default setup form with a centered repository
welcome/create dialog and human summaries. F3 adds models, F4 explicitly exposes
advanced details, and separate confirmation still precedes file creation.
Screen questions are removed; shared dialog identity/RGB and fallback modes remain. `fluzo --safe-screen-settings` provides invocation-only
ASCII/no-color/FPS-0/reduced-motion recovery without saving emergency values.
The requested MVP devmenu default is true; ordinary Developer menu > Presentation
settings opens the typed editor, not synthetic actions. Explicit disable is retained.
Repeated resize PTYs preserve dimensions and prohibit window-resize sequences,
but the reported Ghostty/Wayland physical resize reset is not yet reproduced.
The F3 model wizard reuses centered dialogs: Local/Frontier, provider, optional
Authorization environment reference, URL blur query behind a cleartext warning
for non-loopback endpoints, and multiple model selection.
Core discovery protocol 3 carries only references and optional reported metadata;
the runtime resolves headers for the selected endpoint. No secret is saved in repo
configuration. No startup probes, redirects, retries or inference are enabled.
It accepts localhost, private IPs and DNS names over HTTP, not HTTPS. The worker
resolves names itself and keeps the original authority in the Host header for
virtual-host routing; locality remains the operator's responsibility, but a
non-loopback endpoint now needs an explicit per-run acceptance before any socket
is opened, and the accepted address class is rechecked at connect. See
CONFIGURATION.md for limits.
Pinned cached Hyper/Tokio/JSON dependencies now have runtime production paths;
core/TUI remain HTTP-free. Focused checks are `cargo test -p fluzo-runtime --locked
--offline model_discovery`, `cargo test -p fluzo-tui --locked --offline model_wizard`
and `python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'`.
GitHub provider work remains deferred to a later story.
UI-02 C3 developer-menu route wiring is evidenced by route-level shared-service
integration tests (see the C3 section in TUI.md); S3 human visual/parent
acceptance remains pending.
RUN-05 adds the per-process inference admission layer: pure pool/queue/budget
rules in `fluzo-core::admission` and the live `AdmissionController` in
`fluzo-runtime::admission` with FIFO foreground queues, shadow skip, deadlines
bounded by remaining task time, uncertain-slot reconcile and drain-on-lower
semantics. See [ADMISSION.md](ADMISSION.md). Focused checks are `cargo test -p
fluzo-core --locked --offline admission`, `cargo test -p fluzo-runtime --lib
--locked --offline admission` and `cargo test -p fluzo-runtime --test
admission_simulator --locked --offline` (strict loopback fixture: 429 without
retry, redirect never followed, timed-out waits sending nothing). The agent
execution loop that calls the controller is still unimplemented. AGT-01 adds
the generative provider gateway in `fluzo-runtime::providers`: a pure SSE
framing parser, a Chat Completions assembler that JSON-validates streamed
tool-call argument buffers before any call is dispatchable, and
`ProviderGateway` composing consent, admission, bounded non-quota transient
retries, cancellation and pinned GenAI telemetry over a hyper streaming
client. Redirects are blocked with zero traffic to the Location target, 429
never retries, a break after any surfaced delta is `partial_stream` that is
never retried and leaves the admission slot Uncertain, and cancellation after
dispatch is likewise Uncertain until explicit reconciliation. Credentials are
env references only and never reach telemetry or errors. See
[PROVIDER_GATEWAY.md](PROVIDER_GATEWAY.md). Focused checks are `cargo test -p
fluzo-runtime --lib --locked --offline providers` and `cargo test -p
fluzo-runtime --test provider_gateway_simulator --locked --offline`; the
simulator tests use gates between chunks because the fixture's scripted
disconnect may abort before the response head flushes. The agent loop, CLI
wiring and settings mapping that call the gateway are still unimplemented.
AGT-02 adds the context engine: a pure ledger in `fluzo_core::context`
(typed trust tiers, mandatory provenance, estimates that never impersonate
provider usage, deterministic assembly that blocks on required content and
records every omission), deterministic workspace discovery in
`fluzo_runtime::context` (fixed PRD 16 recipe, tracked-only and
secret-bearing exclusion enforced against Git's own index, with every
inclusion, skip and diff redaction recorded), explicit-only skills
(`ExplicitSkillSelection` is buildable solely from ids, so implicit
activation is structurally unavailable), and a bounded, observable,
reversible compaction engine in `fluzo_runtime::compaction` (segment count
fixed before the first call, a closed blocked-reason set with no `Other`,
the summary as non-authoritative `ModelText`, required items and runtime
records never compactable, and an atomic `commit_unit` journal record that
names what the summary replaced). Compaction rides the AGT-01 gateway as
`RequestClass::Compaction` and checks cancellation before each call, never
during. See [CONTEXT.md](CONTEXT.md). Focused checks are `cargo test -p
fluzo-core --lib --locked --offline context`, `cargo test -p fluzo-runtime
--lib --locked --offline context` and `cargo test -p fluzo-runtime --lib
--locked --offline compaction`. The agent loop that calls discovery and
compaction, the production transport adapter that hands the gateway a live
`ChatRequest`, and any TUI view of pack composition are still
unimplemented.
Agent execution, production task transport, full TUI workflows, providers,
session persistence and runtime acceptance are not implemented. The
architecture and test pyramid below describe requirements beyond this bootstrap;
passing foundation checks does not establish MVP behavior or performance.

Read only the relevant specification sections for the owning issue:

- [PRD v0.2](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/PRD.md): product authority; sections 34 and 53 define MVP scope and acceptance.
- [Architecture v0.1](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/ARCHITECTURE.md): accepted decisions A01-A10, ownership and failure contracts.
- [Delivery plan](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/GITHUB_ORGANIZATION.md): seed scope and source-of-truth rules.
- [GitHub issues](https://github.com/fluzo-labs/fluzo/issues): current dependencies, acceptance and delivery status. Recheck rather than copying status into this file.
- [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and the PR template govern contributions.

The links above are the immutable design baseline used by the implementation
issues. Check an issue for explicitly approved superseding revisions. Product
scope, security guarantees, defaults and acceptance changes require review in
`fluzo-docs`; a generic skill or convenient library default cannot change them.
Implementation APIs, verified commands and tests belong here, not in a duplicate
schema maintained in the documentation repository. Project documentation, issues,
PRs and application-generated UI/help/diagnostics use English; user/model/tool
content retains its original language.

## Delivery order and scope

Start with FND-01 (#1): four-crate workspace, MIT metadata, committed lockfile,
recorded toolchain/MSRV and dependency-boundary checks. Then implement the typed
settings registry (#2), application port/scenario driver (#3), initial CI (#4)
and HTTP simulator (#5) according to their prerequisites.

The milestone sequence is foundation, reviewed visual prototype, diagnostics and
storage, safe execution, native agent, then integration and release. Independent
work may proceed once its prerequisites are met. Split large briefs before
coding; DAT-02 (#12) explicitly requires child issues. Track each applicable
PRD section 53 requirement to an owning issue and evidence throughout delivery,
not only during the final acceptance-matrix issue (#25).

The MVP targets Linux x86_64 on Arch Linux and CachyOS, with Ghostty and Alacritty.
It edits the current Git working tree with one native agent, an OpenAI-compatible
Chat Completions provider and optional Laya shadow observations. Active routing,
ACP/MCP, multi-agent teams, isolated task worktrees, third-party plugin loading,
macOS and Windows are later work. Cross-instance inference coordination and
provider rate-limit management are excluded from the current roadmap as well.
Do not create placeholder crates, services or extension frameworks for them.

## Architecture and dependency boundaries

Use a modular monolith with an embedded runtime, not microservices:

| Crate | Responsibility | Forbidden coupling |
| --- | --- | --- |
| `fluzo-core` | Owned/versioned protocol DTOs, IDs, states, config definitions/metadata, pure policy and budget rules | Filesystem/network I/O, SQLite connections, terminal widgets, other Fluzo crates |
| `fluzo-runtime` | Coordination, sessions, persistence, context, providers, tools, admission, telemetry and platform adapters | TUI widgets, focus, layout or input handling |
| `fluzo-tui` | View state/reducers, composer, settings controls, sanitized rendering and animation | Runtime implementation imports, direct DB/provider/tool access |
| `fluzo-cli` | Binary, startup mode, argument parsing, dependency wiring, headless presentation and shutdown | A second agent loop or policy implementation |

Production dependencies are `runtime -> core`, `tui -> core`, and
`cli -> {core, runtime, tui}`. Validate the resolved Cargo graph, including
supported production feature combinations. Runtime modules may initially include
`application`, `config`, `sessions`, `execution`, `admission`, `context`,
`providers`, `tools`, `storage`, `telemetry` and `platform`; these are suggested
internal modules, not additional crates to scaffold.

The CLI supplies a transport-neutral application port defined in core. Commands,
queries, snapshots and subscriptions carry owned serializable values, IDs,
versions and cursors, not DB handles, callbacks, channels, cancellation tokens or
shared mutable domain objects. Concrete transport/lifecycle ownership stays
outside the TUI. An acknowledgement is not operation completion.

Tokio, Ratatui/Crossterm, Clap, Serde/toml_edit, Reqwest, rusqlite, rustix and
tracing/OpenTelemetry are architecture candidates. Exact versions, feature flags,
MSRV and integration details must be validated, not inferred from skill examples.
Axum is a candidate for the test HTTP simulator, not a new production service.

### Execution and data flow

1. An explicit user command starts or resumes a task through the application port.
2. Runtime checks trust/config, workspace ownership and task state, then persists
   the attempt and effective configuration.
3. Context is bounded and, when necessary, compacted through the same provider
   gateway used by all inference paths.
4. Admission atomically reserves applicable task budgets and model/pool capacity.
5. Provider adapters assemble and validate complete responses/tool calls. Model
   output proposes work; it never calls tools directly.
6. Runtime rechecks permissions, source versions, cancellation and budget. It
   commits operation intent and one-time grant consumption before dispatch.
7. Tools execute; runtime persists results/artifact references and publishes
   sanitized authoritative updates. Unknown outcomes remain explicitly unknown.

Keep task, attempt, operation and provider-request state distinct. Use monotonic
elapsed time for deadlines. Durable events and materialized state commit together;
streaming presentation updates may be bounded/coalesced but are not audit records.
Snapshots/subscriptions need a consistent cursor handoff, gap detection and
resynchronization. Neither opening a view nor reconnecting may execute work.

## Non-negotiable implementation practices

### Persistence and ownership

- One runtime owner and one writing task per workspace across local processes.
  A PID alone is not ownership proof. Read-only inspection/control clients are
  separate; detaching one is not owner shutdown.
- Owner exit initiates bounded shutdown; the MVP does not keep a daemon running.
  Cancel and confirmed force stop are distinct, limited to verified owned work.
  Preserve partial edits and uncertainty; never automatically roll back.
- Store repository history in `.fluzodrive/state.sqlite3`, with a serialized
  writer, verified WAL, `synchronous=FULL` and foreign keys. Keep blocking SQLite
  work off the input/event loop. Never hold a transaction across a human wait,
  inference, tool execution or capacity wait.
- Durable intent acknowledgement precedes every side effect. A crash after an
  effect but before its result commit does not make retry safe. Recovery requires
  reconciliation and explicit resume; no automatic operation replay.
- Artifact finalization and SQL reference commit are separate durability steps.
  Use SQLite-aware migration backups, including WAL state. Retained data does not
  silently expire; quotas bound admission/capture, not physical filesystem usage.
  Purge needs preview, confirmation and active/shared-reference checks, even in YOLO.

### Configuration, permissions and external I/O

- `.fluzo` is a TOML file, not a directory. A single typed core registry supplies
  defaults, validation and TUI metadata. Runtime revalidates all writes. Saving
  future defaults and applying changes to active work are separate operations;
  neither resets consumed budgets. Preserve CLI precedence and external edits.
- Repository configuration is data, not consent. Credentials/consent are user-local
  and separate from repository history; there is no inherited global runtime
  configuration. Never commit private endpoints, credentials, `.fluzo`, model
  weights or `.fluzodrive/`; exclude private history from context independently of Git.
- Denials win. Grants have exact scope, lifetime and consumption. Session YOLO
  bypasses prompts, not denials, budgets, workspace checks or cancellation, and
  never returns automatically after recovery. Headless approval requirements
  return a structured outcome promptly instead of waiting for terminal input.
- Native file confinement uses race-aware root-relative handles and version
  checks, not string prefixes or canonicalize-then-open. Fail closed on unsupported
  containment. Direct commands and explicit shells are not an OS sandbox.
- Spawn with explicit executable/arguments, working directory and reviewed
  environment; close stdin and do not inherit provider secrets, a controlling
  terminal or implicit Git helpers. Never fall back to a shell after spawn failure.
  Continue bounded pipe draining after capture truncation to avoid deadlocks.
- All application HTTP transports, including telemetry, reject redirects. HTTP
  429 and recognized capacity/quota rejections receive no automatic retry,
  cooldown or provider fallback. Other transient retries need bounded, safe classification.
- Do not stage, commit, push, stash, reset or create task branches/worktrees
  automatically. Preserve staged, unstaged and untracked user work.

### Async, capacity and presentation

- Prefer explicit ownership, typed state/errors and narrow capabilities over
  generic executors or pervasive shared mutable state. Domain errors must remain
  machine-readable, including permission, budget and uncertain-outcome categories.
- Keep blocking/expensive work off input/control paths. Bound workers, queues,
  capture and caches by relevant counts and bytes. Cancellation/control must not
  wait behind an unlimited stream of tokens or notifications.
- Dropping a future is not proof that remote inference or an OS process stopped.
  Do not unconditionally release dispatched capacity through RAII; unresolved
  work retains uncertainty until completion evidence or explicit reconciliation.
- Model/pool limits are atomic and per process; workspace ownership is cross
  process. Do not confuse these scopes. All inference, including compaction and
  observations, goes through one gateway. With the default one-slot pool and one
  reserved foreground slot, Laya observations are skipped, not run in parallel.
- Laya recommendations never authorize, route or execute actions. Context and
  summaries retain provenance and cannot become grants. Compaction failure blocks
  safely instead of losing history or looping indefinitely.
- Use one terminal writer, viewport-bounded rendering, revision-aware caches and
  independent input/animation clocks. Zero animation FPS must still accept input
  and cancellation immediately. Idle/headless mode has no animation loop.
- Sanitize untrusted terminal content incrementally, including split escape
  sequences and replay. Secret redaction is a separate layer. Preserve focus,
  drafts and scroll anchors; incoming output must never approve a pending action.
- Required SQLite audit, local JSONL diagnostics and opt-in OTLP are separate
  paths. Audit failure blocks dependent effects; diagnostic sink failure exposes
  bounded loss/degradation without blocking task control. No task IDs in metric
  labels and no per-token/per-frame telemetry.
- Profile before specialized optimizations. Do not copy `panic = "abort"`, native
  CPU flags, allocators or broad feature matrices from a skill without validating
  terminal restoration, supported CPUs, cancellation and actual performance.

## Test pyramid and evidence

This is the target pyramid, not an existing suite. Prefer many fast pure tests,
focused boundary/integration tests, and fewer full-system scenarios. Do not impose
an invented percentage or replace critical process/HTTP tests with unit mocks.

| Layer | Required focus |
| --- | --- |
| Pure unit tests, widest base | State transitions, policy precedence, budgets, queue decisions, config cross-field validation, TUI reducers; controllable clocks/IDs; no terminal, DB or network |
| Contract and component tests | DTO round trips, port acknowledgement/cursors/gaps, config defaults/save/apply, incremental sanitization, buffer snapshots; TUI fixtures do not import runtime |
| Adapter and integration tests | Real temporary SQLite/filesystems/processes, crash boundaries, confinement races, migrations/purge, ownership/control sockets; scripted loopback HTTP streaming/errors/cancellation |
| Deterministic native E2E | Actual native loop, policies, tools, storage and telemetry repair a disposable Rust fixture via HTTP simulators; independent verification and unchanged tests |
| Reference/manual acceptance | PTY interactions plus Ghostty/Alacritty review, controlled performance runs, Arch/CachyOS packaging and dedicated Compose diagnostics |
| Optional live evaluation | Explicit authorized model configuration and finite budgets; report separately, never a required CI/build/package gate |

Use normal Rust unit/integration tests as the starting point; snapshot/property/
model-checking/benchmark libraries need deliberate selection, not automatic
installation from a skill. Add a focused regression for changed behavior and test
negative paths, not only success. Use barriers/acknowledgements rather than
arbitrary sleeps; OS/PTY tests still need bounded real-time safety deadlines.

Simulators must reject unexpected requests and missing required steps. They only
return protocol data: the runtime performs actual edits/tools. Isolate home,
config, proxy/credential environment, ports and repositories. Only listeners
registered for that run are valid provider destinations, not arbitrary localhost
services. Never probe or fall back to real models during deterministic tests.

The Rust repair fixture compiles before editing, fails on the intended assertion,
then passes independent harness verification without weakening tests or build
configuration. Preserve all failed attempts in reports. Include denial,
partial-edit cancellation, user changes, offline Laya/Collector, redirect targets
receiving zero requests, and 429 without automatic recovery.

Normal deterministic tests use a local telemetry receiver and require no Docker.
A dedicated Docker-capable job verifies Collector/Tempo/Prometheus/Loki/Grafana,
all three signals, correlation and outage behavior. Unselected live tests are
`not_run`; selected but failed live tests must not silently become simulator runs.

Buffer snapshots do not prove visual quality or terminal performance. Test
80x24, 120x40 and 160x50 layouts, resize/paste/focus, sanitization and terminal
restoration. PRD section 49 defines reference workloads and proposed performance
gates; establish/review profiles early, measure on controlled machines and never
claim shared-CI timing or application redraw cadence proves physical display FPS.

## Commands and validation status

Currently available:

```sh
git status --short
git diff --check
gh issue view 1 --repo fluzo-labs/fluzo
gh issue list --repo fluzo-labs/fluzo --state open --limit 100
```

Rust 1.98.0 is pinned in rust-toolchain.toml with rust-src, rust-analyzer,
Clippy and rustfmt. The conservative initial MSRV is 1.98, matching the tested
toolchain rather than claiming compatibility with untested older releases.
Edition 2024 and resolver 3 are selected; unsafe code is forbidden by workspace
lint. FND-02 introduces pinned Serde and toml_edit with explicit features and a
reviewed transitive graph. Workspace feature declarations remain unsupported.
Fresh machines prepare build dependencies with `cargo fetch --locked` before the
offline checks; this is separate from inference and requires registry access.

Verified foundation commands, mirrored in `.github/workflows/development.yml`:

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
cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo
```

The binary supports bootstrap help/version, a static protocol demo and
`demo --interactive`, a synthetic TUI shell. The interactive shell queries the
application port supplied by CLI; it never executes tasks or reads `.fluzo`.
Current keys follow Crush for implemented demo actions: Enter sends a retained
preview, Ctrl+J/Shift+Enter inserts a newline, Esc cancels playback, Ctrl+C exits,
Ctrl+G shows help and Ctrl+B toggles the wide sidebar. Ctrl+S now opens read-only
session information, not draft preview. See TUI.md for unavailable shortcuts and
terminal encoding limits; key-map alignment is not full workflow parity.
Focused checks are `cargo test -p fluzo-tui --locked --offline` and
`python3 -m unittest discover -s scripts -p 'test_tui.py'` (Linux PTY).
Ratatui 0.29.0/Crossterm 0.28.1 and Unicode/signal helpers are pinned; terminal
libraries are permitted only in TUI/CLI production paths. The graph checker
validates multiple reviewed versions separately. PTY tests join ordinary Python
unittest discovery in CI. Visual review and reference performance are unverified.
Execution returns failure and explicitly says the runtime is not implemented.
The demo does not read configuration or execute tools; scenario advancement is
host-controlled, never a side effect of inspection, polling or reconnecting. Python 3.11+ is needed for tooling
checks. The graph checker permits only the reviewed Serde/TOML dependency graph,
versions, registry source and feature ceilings. SIM-01 adds pinned dev-only
Hyper/Tokio/JSON dependencies and their reviewed transitive graph, with no new
production paths. DAT-01 adds `sqlx 0.9.0` (runtime-tokio, sqlite-bundled) as a
reviewed `fluzo-runtime` production path with its full transitive closure in a
`(name, version)`-keyed policy; rusqlite is not in the offline cache and raw
`libsqlite3-sys` use is ruled out by the workspace `unsafe_code` prohibition.
Focused store checks: `cargo test -p fluzo-runtime --locked --offline store`.
DAT-02a adds `sha2 0.11.0` (alloc-only) as a reviewed `fluzo-runtime`
production path for artifact content hashing, with its closure
(digest 0.11.3, crypto-common 0.2.2, hybrid-array 0.4.15, cpufeatures 0.3.1,
block-buffer 0.12.1) in the same `(name, version)`-keyed policy; the older
sha2 0.10.9 graph remains only through sqlx. Focused artifact checks:
`cargo test -p fluzo-runtime --locked --offline artifact`.
Core/TUI cannot import TOML;
forbidden Fluzo paths remain checked transitively through normal/build edges.
The LSP fixture vendors only locked cached dependencies into its disposable root,
keeping an empty HOME/CARGO_HOME and no inherited user configuration.
Metadata CI checks community files; development CI checks foundation and HTTP
simulation, including a private network namespace with loopback only.
`python3 scripts/test_http_simulator.py` uses an unprivileged user/network namespace
locally. CI explicitly uses `--ci`, with noninteractive sudo for network namespace
setup and `setpriv` to drop identity/groups/capabilities before tests, avoiding
restricted user-ID mapping on Ubuntu 24.04. Both modes fail closed, never fall back
to host networking, and report live inference as `not_run`. CI mode requires
`sudo`, `unshare` and `setpriv`; see HTTP_SIMULATOR.md for verification limits.
Focused protocol checks use
`cargo test -p fluzo-runtime --test http_simulator --locked --offline`.
Runtime E2E, benchmarks and packaging are still unimplemented. CI commands passed locally;
a remote workflow run is not implied.

S3 focused checks use `cargo test -p fluzo-cli --test configuration --locked --offline`,
`cargo test -p fluzo-tui --locked --offline configuration`, and
`python3 -B -m unittest discover -s scripts -p 'test_configuration.py'`.
They compose actual controlled-host UI/service persistence, unit projections and
ordinary-host PTYs separately. S3 does not add dependencies or a task runtime.
The normal shell uses Ctrl+P; F1 explains settings controls and F4 toggles the
editor between the registry's basic and advanced field tiers. `cargo test -p
fluzo-core --locked --offline tiers` pins the registry-owned tier split. Setup F2
handoff is explicit. Service exhaustion retains outcomes and drafts without automatic replay
or replacement; reopening explicitly loses unsaved local drafts.

S1 configuration checks use
`cargo test -p fluzo-runtime --lib --locked --offline configuration`.
S2 setup checks use
`cargo test -p fluzo-runtime --lib --locked --offline setup`,
`cargo test -p fluzo-tui --locked --offline setup`, and
`python3 -B -m unittest discover -s scripts -p 'test_setup.py'`.
The service uses a separate core configuration port and one bounded runtime
worker; it does not extend task IDs or wire the demo to user configuration.
Replacement requires an explicitly supplied LocalWorkspace policy; exclusive
creation uses CreateOnly and never upgrades itself to replacement. Arbitrary
external editors and network filesystems are not covered by the coordinated
replacement contract, which is why the worker polls for outside changes and Save
rebases onto the file actually on disk. Setup visual acceptance remains a human
gate. Outside-change checks use
`cargo test -p fluzo-runtime --lib --locked --offline configuration`,
`cargo test -p fluzo-tui --locked --offline configuration` and the PTY case
`test_outside_change_banner_offers_keep_ours_then_reload` in
`scripts/test_configuration.py`.

RUN-01 focused checks use
`cargo test -p fluzo-runtime --lib --locked --offline workspace` and
`cargo test -p fluzo-runtime --lib --locked --offline confinement`. The
ownership fixtures spawn real child processes through a pipe barrier, so
they need no sleep-based synchronization; the child role test
`workspace::tests::child_role` is `#[ignore]`d and only runs when
spawned by its parent. These tests use
the real filesystem and real process locks, not mocks, and require no
network. Running `cargo test -p fluzo-runtime --lib --locked --offline
child_role` alone proves nothing.

RUN-02 focused checks use
`cargo test -p fluzo-core --lib --locked --offline policy`,
`cargo test -p fluzo-runtime --lib --locked --offline authorization` and
`cargo test -p fluzo-runtime --lib --locked --offline session_auth`. The
headless non-blocking contract is proven by the child process in
`session_auth_tests::headless_child`, which receives a stdin pipe the
parent keeps open and never writes to; the child must decide and exit
without reading it, and the parent fails on deadline rather than trusting
a generous timeout. That child is `#[ignore]`d and only runs when spawned
by its parent; running it alone fails its role assertion. These tests are
pure or process-based and need no network.

Run the narrowest relevant tests after a change, then the applicable workspace
checks. Define supported feature combinations explicitly rather than assuming
`--all-features` is valid or safe. Keep dependency preparation separate from
inference-free checks; `--locked` is not an offline/network-isolation guarantee.
Record actual commands/results and mark unavailable checks unverified. Update
this list when subsequent milestones introduce commands or dependencies.

## Concurrent agent operation

Local inference is the binding constraint on how many agents run at once, not
CPU, memory or pane count. The operator's measured Halo Strix figures are
about 16.1 tok/s decode for a single request and about 12.38 tok/s per
request at two concurrent requests. Four slots are enabled and were proven at
roughly 7,800 input and 512 output tokens per request. Four long-running
coding agents, and four full 128K conversations, are not proven. Consistent
with the per-process admission rules, `/v1/models` and context length do not
establish serving slots: these are operator-confirmed numbers and they move
with the runtime, model and server configuration.

Working ceiling: three concurrent lanes. Two autonomous workers plus one human
lane, leaving one slot as headroom for verification runs and bursts. Running
four saturates the server and slows the interactive session that reviews the
work, which is the session that actually gates delivery. The small model
carries titles and summaries off the local box, and that is what makes several
Crush instances tolerable; it is not a local slot.

Lane layout inside the Herdr workspace:

```text
workspace <repo>
├── p1  orchestrator    human lane, keeps focus
├── p2  <repo>-<phase>  worker on the active phase
└── p3  <repo>-<area>   optional second worker
```

One workspace per repository. Splits take an explicit `--cwd` and `--no-focus`
so the human's focus never moves. Each worker is renamed to
`<repo>-<phase>`, because every Crush instance reports the same detected agent
label and the sidebar is otherwise unreadable.

Dispatch contract. Long instructions go in a file, never through keystrokes:

```bash
export HERDR_ENV="$(tr '\0' '\n' < /proc/$$/environ | sed -n 's/^HERDR_ENV=//p')"
export HERDR_SOCKET_PATH="$(tr '\0' '\n' < /proc/$$/environ | sed -n 's/^HERDR_SOCKET_PATH=//p')"
herdr pane send-text <pane> "Read /tmp/<task>.md and execute it end to end."
herdr pane send-keys <pane> return
herdr pane wait-output <pane> --regex "github\.com/[^ ]+/pull/[0-9]+" --timeout 900000
```

The task file carries everything the worker needs so it never has to ask:
repository, branch and HEAD, plan path, issue, the maintainer's verbatim
acceptance directive when one exists, the exact verification commands, the
pull request rule (`Ref` rather than `Closes` while any criterion stays open)
and the required final report.

Phase gates keep the meaning the plan format gives them. `implemented`,
`verification` and `review` are distinct states and are never collapsed into
one. A criterion that lives in another repository keeps
`has_completed_all_phases` at `false`, and the pull request references its
issue instead of closing it.

Autonomy boundary. An agent may search, read, implement, verify, commit, push,
open a pull request and update the plan with real evidence without asking. A
human must move `review` to `accepted`, merge, delete anything, probe a
non-loopback endpoint, change configuration, and supply the formal approval
from a second GitHub account.

Known environment limitations, each with the workaround in use:

| Limitation | Workaround |
| --- | --- |
| Crush's shell strips `HERDR_ENV`, `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` from child environments although they exist in the process environment | re-export them from `/proc/$$/environ` before calling `herdr` |
| `herdr agent prompt`, `wait` and `get` reject Crush with `not an active named agent` | drive the pane with `pane send-text` plus `send-keys return` |
| `herdr agent start --kind` has no Crush kind | launch with `pane run` |
| GitHub rejects approving your own pull request | post the review as a comment and merge by hand or from a second account |
| `gh pr diff` has no per-path filter | use `git diff origin/main...HEAD -- <path>` |
| Switching a shared checkout to another branch strands a worker mid-task | give each concurrent change its own `git worktree` |

The queue that actually accumulates is human review, not the GPU. With four
slots and two or three lanes the hardware is not the constraint; visual
acceptance, live-environment reproduction and release evidence are. The
efficient pattern keeps two workers producing while the maintainer drains
gates rather than code.

## Skills and contribution completion

See [SKILLS.md](SKILLS.md) for researched candidates, revision/license notes,
known conflicts and adoption status. Twelve skills live in `.agents/skills`.
Five come from `fluzo-labs/fluzo-skills`: `rust-practices`, `rust-review`,
`fluzo-deterministic-testing`, `fluzo-rust-boundaries` and `tui-design`, pinned in
`.agents/fluzo-skills.toml`. All same-name local versions were explicitly replaced
by the Fluzo collection versions; do not restore them or install duplicate copies
elsewhere. The Rust adaptations retain their historical leonardomso/Apollo origins
and licenses; their direct installation source is Fluzo. Complete references and
upstream provenance are retained; rust-review remains Apache-2.0, not MIT.

Seven additional workflow skills come from `fluzo-labs/common-skills`:
`issue-refine-github`, `plan-create`, `plan-execute`, `delivery-review-github`,
`convention-document`, `git-conventional-commit` and `release-prepare-github`.
Their complete folders, references and MIT licenses are copied locally from
release v1.2.0; the pinned revision and SHA-256 inventory live in
`.agents/common-skills.toml`. All seven include a self-contained bounded autonomous
contract: explicit finite scope, per-operation authorization, delegated content,
shared limits and a stop at review. Guided mode remains the default; installing
this update does not activate autonomy or authorize commits/publication. Existing
project and higher-priority approval requirements remain in force.
Crush discovers this directory by default, without another skill-path setting.
Load only the matching entry point and required references. Project rules and
higher-priority instructions prevail over generic workflow advice, including
language, attribution and publication requirements. Installation grants no
permission to commit, push, modify GitHub or release. GitHub remains the backlog
authority; a local plan must not become a second status board.

Project `crushrc` registers rust-analyzer through the pinned rustup toolchain,
without changing permissions or providers. After reopening the project, Crush
recognized the project configuration and the initial four skills. The seven
common skills and the Fluzo collection migration were installed afterwards and
may require another project reload. Its Rust LSP became
ready on source access; references across core/runtime/TUI and document symbols
worked, with no reported diagnostics. The definition tool found the symbol but
omitted its path, so use references/symbols to confirm navigation locations.
The direct LSP fixture separately verifies cross-crate rename and compiler errors;
rename through Crush and model-driven skill calibration remain unverified.
Crush reports a ready `rust_analyzer` client while the explicit `rust-analyzer`
registration remains `not_started`; do not infer which registration launched it.
Review Cargo configuration, dependencies, build scripts and proc macros before
LSP indexing: it can execute code and is not a sandbox. No application runtime
LSP or skill-loading capability is implied by these development tools.

Before completing a change: link its issue and design sections, verify dependency
boundaries and safety invariants, run applicable tests, inspect the diff for secrets
and unrelated changes, and document actual evidence plus remaining limitations.
Use PRs and recorded self-review per CONTRIBUTING; do not claim release acceptance
from a compiling scaffold. Publication/tagging requires explicit maintainer approval.
