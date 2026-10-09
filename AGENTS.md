# Agent guide

## Status and authority

Fluzo is a local-first coding-agent runtime in Rust at the development-bootstrap
stage. The four-crate workspace, bootstrap help/version binary, pinned toolchain,
local skills, development checks and the FND-02 typed configuration library exist.
See [CONFIGURATION.md](CONFIGURATION.md) for settings APIs and limits. FND-03 adds
an owned application protocol and an in-memory scenario driver; see
[APPLICATION.md](APPLICATION.md). SIM-01 adds test-only strict HTTP/SSE fixtures
and a Linux private-network profile; see [HTTP_SIMULATOR.md](HTTP_SIMULATOR.md).
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
basic/advanced editing with protocol 5, owned descriptors, operation
availability and bounded request reconciliation. Presentation Apply is separate.
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
Authorization environment reference, URL blur query and multiple model selection.
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
Full UI-02 C3 menu wiring and S3 human visual/parent acceptance remain pending.
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
production paths. Core/TUI cannot import TOML;
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
The normal shell uses Ctrl+P; F1 explains settings controls. Setup F2 handoff is
explicit. Service exhaustion retains outcomes and drafts without automatic replay
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

Run the narrowest relevant tests after a change, then the applicable workspace
checks. Define supported feature combinations explicitly rather than assuming
`--all-features` is valid or safe. Keep dependency preparation separate from
inference-free checks; `--locked` is not an offline/network-isolation guarantee.
Record actual commands/results and mark unavailable checks unverified. Update
this list when subsequent milestones introduce commands or dependencies.

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
