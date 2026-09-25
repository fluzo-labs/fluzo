# Agent guide

## Status and authority

Fluzo is a local-first coding-agent runtime in Rust at the development-bootstrap
stage. The four-crate workspace, bootstrap help/version binary, pinned toolchain,
local skills and development checks exist. Agent execution, the application port,
TUI, providers, persistence and runtime acceptance are not implemented. The
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
Edition 2024 and resolver 3 are selected. There are no external Cargo dependencies
or feature combinations yet; unsafe code is forbidden by workspace lint.

Verified foundation commands, mirrored in `.github/workflows/development.yml`:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
cargo build --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_lsp.py
cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help
```

The binary only supports bootstrap help/version; execution returns failure and
explicitly says the runtime is not implemented. Python 3.11+ is needed for tooling
checks. The graph checker deliberately rejects external production dependencies
and features until their policy is reviewed with their first implementation.
Metadata CI checks community files; development CI checks foundation only. Runtime
E2E, benchmarks and packaging are still unimplemented. CI commands passed locally;
a remote workflow run is not implied.

Run the narrowest relevant tests after a change, then the applicable workspace
checks. Define supported feature combinations explicitly rather than assuming
`--all-features` is valid or safe. Keep dependency preparation separate from
inference-free checks; `--locked` is not an offline/network-isolation guarantee.
Record actual commands/results and mark unavailable checks unverified. Update
this list when subsequent milestones introduce commands or dependencies.

## Skills and contribution completion

See [SKILLS.md](SKILLS.md) for researched candidates, revision/license notes,
known conflicts and adoption status. Four project skills live in `.agents/skills`:
`rust-practices`, `rust-review`, `fluzo-rust-boundaries` and
`fluzo-deterministic-testing`. The first two are pinned, licensed adaptations;
the latter two are original procedures. Load only relevant references.

Project `crushrc` registers rust-analyzer through the pinned rustup toolchain,
without changing permissions or providers. After reopening the project, Crush
recognized the project configuration and all four skills. Its Rust LSP became
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
