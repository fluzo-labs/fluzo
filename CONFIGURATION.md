# Typed configuration foundation

This implements the library scope of [FND-02 (#2)](https://github.com/fluzo-labs/fluzo/issues/2),
using [PRD v0.2 sections 26, 27 and 43](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/PRD.md)
and [architecture A06, section 10](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/ARCHITECTURE.md).
It does not implement a wizard, configuration CLI, application port, file saving,
active-task application, credentials service, provider or agent execution.

## Ownership and APIs

- `fluzo_core::settings::Settings` owns the versioned typed configuration.
  Its Rust field declarations supply defaults, semantic descriptors and field
  constraints together. There is no independent default list for the UI.
- `Settings::entries()` pairs descriptors with current values;
  `descriptors()` exposes metadata alone. `collection_descriptors()` supplies
  templates for model/pool creation. Dynamic names are explicit schema-defined
  collections, not a catch-all for arbitrary settings. Names use 1-128 ASCII
  alphanumeric, underscore or hyphen characters.
- Descriptors carry type/options, optionality, integer maximum, constraint, unit,
  description, privacy and application rule. `Presentation`, `FutureTasks`,
  `ExplicitApply` and `Restart` describe effects; they never perform those effects.
  `SettingOrigin` defines provenance categories for later application snapshots.
- `Settings::validate()` is pure and returns key/category errors without values.
  It checks the schema version, ranges, model/pool references, authentication
  reference combinations, context/output reserves, budgets, timeouts, capture
  quotas, alias capacity consistency and tool environment declarations.
- `fluzo_runtime::config::{parse_settings, encode_settings}` adapts strings to and
  from TOML and always calls core validation. Explicit files must declare
  `schema_version = 1`; omitted supported fields use the typed defaults. Unknown
  keys, invalid types/choices and malformed TOML fail instead of being ignored.
- `update_draft` edits an existing registered key in an in-memory TOML document,
  preserves unrelated comments/values and validates the result. It never writes
  a file or modifies the caller's settings. Atomic save, external-version checks,
  multi-field UI drafts, CLI precedence and active budget preservation belong to
  the subsequent application/settings workflow, not this string adapter.
- `redacted_values` is the diagnostic projection: sensitive fields and credential
  references are replaced by redaction markers. Raw settings and serialized TOML
  are private configuration, not safe logging formats.

Core uses Serde but no filesystem, environment, network, database or terminal
operations. Runtime owns TOML parsing. TUI consumers can use core metadata and
validation without importing runtime or the TOML parser.

## Visual session preferences

The isolated `demo --interactive` shell exposes the presentation subset below.
The shared core registry supplies its descriptors, defaults, choices and validation;
the menu does not import runtime configuration parsing. This catalog follows the
approved D0 design revision `53b345e9f1782beef839b42b4c0de4773ed371df`, PRD 29.2.2.
The table describes the current implementation, not a separate schema.

| Stable key | Purpose | Choices or bounds | Default | Application |
| --- | --- | --- | --- | --- |
| `tui.theme` | Select the reviewed palette | `default`, `high-contrast` | `default` | Live presentation |
| `tui.animation_fps` | Cap animation-driven redraws | Integer 0 through 60 | 60 | Live presentation |
| `tui.reduced_motion` | Disable animated motion | Boolean | Off | Live presentation |
| `tui.flags.render_diagnostics` | Show aggregate rendering diagnostics | Boolean | Off | Live presentation |
| `tui.notifications.desktop_enabled` | Explicit desktop opt-in | Boolean | Off | Session Apply; Ghostty only |

The Developer Menu is disabled by default. Start the demo with `--dev-menu` and
open it through the command palette; `--no-dev-menu` wins if both are supplied.
The theme picker exposes only `tui.theme`. The Developer Menu exposes these five
controls, a separate synthetic notification-text editor and an in-app preview.
Neither synthetic action is a persistent setting.
No extra pulse, logo, accent, density or tool-detail selectors are selected by D0.

Typing filters the menu. Up/Down and Tab/Shift+Tab select entries; Left/Right
step FPS by one or switch theme/boolean values. FPS steppers stop at 0 and 60.
The selected entry shows its key, purpose, default, preview-effective value,
source and live application or CLI lock. No current editable control requires a
restart. Unsupported themes, malformed/out-of-range CLI values and unknown keys
are rejected; a rejected edit does not change applied preferences.

- `BuiltIn` identifies untouched defaults in the current defaults-plus-CLI host.
- `CommandLine` identifies invocation overrides. `--theme`, `--animation-fps` and
  `--reduced-motion` and `--desktop-notifications` lock their corresponding controls
  against edits and Reset.
  The lock message follows the selected entry, including before an edit attempt.
- `Draft` identifies a temporary preview that differs from the applied value.
- `ActiveSnapshot` identifies an explicit session-applied override. Applying a
  reset default is still a session action, not a new configuration-file source.

Ctrl+A applies the preview to this session and increments its UI settings version.
Esc or closing without Apply restores applied values. Ctrl+R previews defaults:
in Theme preview it resets only the theme; in the Developer Menu it resets all
five unlocked preferences, independently of the search filter. Reset is
reversible through Esc. These operations preserve composer text, focus and scroll.
FPS 0 and reduced motion stop animation, not input or static state updates;
terminal color/ASCII capabilities still constrain presentation.

The demo does not read or write `.fluzo`, discover saved preferences or implement
persistent provenance. Save remains unavailable; C3/UI-03 owns the shared save/apply
integration. C2 adds the notification preference and isolated preview.
Desktop test delivery remains separately opt-in and capability/focus gated.
Presentation settings cannot authorize tools, providers, Laya or policy changes.

See [C1 verification in TUI.md](TUI.md#c1-reversible-control-corrections) for the
actual regression failures, final focused test results and remaining manual/runtime
acceptance limits. No product acceptance or configuration persistence is implied.

## Bounded notification settings

The non-visual C2 slice ([#37](https://github.com/fluzo-labs/fluzo/issues/37))
implements the D0 limits from `fluzo-docs` revision
`53b345e9f1782beef839b42b4c0de4773ed371df`, PRD 29.3.1, without changing the demo UI.
The shared registry now bounds `tui.notifications.duration_seconds` to 1 through
30 seconds and `tui.notifications.max_visible` to 1 through 5. Defaults remain
5 seconds and 3 notices; `desktop_enabled` remains false.

Core validation, TOML loading and `update_draft` use the same metadata. Zero,
negative, wrong-type and over-limit values fail with the relevant key; no clamping,
rewriting or automatic migration occurs. Previously accepted out-of-range files
require explicit correction before loading or editing. Rejected draft edits leave
the original source untouched. These limits describe in-app presentation, not
how long a desktop service displays a popup.

The reusable TUI `notification_stack` module validates settings on construction
and atomic reconfiguration. A duration change affects future receipts only;
existing deadlines never restart. Available display capacity is supplied by the
layout owner and may be zero without changing `max_visible`. The component
has no file writer, external notification transport or rendering side effects.
See [the C2 logic contract](TUI.md#c2-notification-logic-without-visual-integration)
for event ordering, memory bounds, expiry and integration limitations.

Desktop enablement is a reversible draft until Ctrl+A applies it for the session.
Unsupported transports reject menu edits; CLI overrides stay locked. Only applied
settings reach the external host: opening, Reset, Cancel and internal previews
never schedule an external send. Applying a disable cancels a pending external
test; re-enabling does not replay it. Explicit Ctrl+T tests and natural synthetic
playback completion retain opt-in, capability and known-blur checks. Applying an
enable does not itself send a notification. No persistence or new transport is added.
Duration and stack-count keys remain editable through the typed configuration API;
no duplicate Developer Menu steppers are introduced.

## Defaults and missing user configuration

The source of truth is the typed declarations in `settings.rs`. Generate a full
non-secret default document with `encode_settings(&Settings::default())`; do not
maintain another static default file. All supported non-optional fields are
emitted. Optional absent fields are omitted because TOML has no null value.

A default configuration deliberately has no model, endpoint, pool or credential
selected. Section 27's URLs and model names are illustrative, not installed
services or discovered user capabilities. `validate()` accepts this offline setup
state; `validate_for_execution()` additionally requires a selected generative
model. Neither validation result establishes consent or runtime readiness.

Model templates retain the 32768-token context suggestion and 4096-token output
reservation, subject to user confirmation of model capabilities. The input safety
reserve is the greater of `context.safety_reserve_tokens` and the rounded-up
`context.safety_reserve_fraction` of the model context. Compaction and capture
limits are validated against response and storage capacity.

The canonical namespaces include project, agent, models, capacity_pools,
decision, harness, tools.shell, policy, context, storage, telemetry and tui
(including notifications and flags). Generative request deadlines are model
fields; Laya uses `decision.timeout_seconds`, without retry controls. Monetary
budgets are optional `harness.max_cost_microunits` plus a three-letter currency;
model input/output prices use integer currency microunits per million tokens.
These are configuration definitions, not implemented accounting or enforcement.

## Safety and diagnostics

- Authentication is `none`, `env` plus `api_key_env`, or `credential` plus a typed
  `credential_ref`. References are never resolved by these APIs. A literal
  `api_key` is unknown and rejected. The credential service will enforce endpoint
  scope when it exists; a syntactically valid reference grants nothing.
- HTTP(S) endpoint syntax excludes userinfo, query/fragment credentials and control
  characters. Non-loopback HTTP still needs separate warned consent at dispatch;
  syntax validation is not destination authorization, DNS resolution or probing.
- Literal strings are not interpolated. Explicit tool environment names cannot
  include configured provider credential variables or reserved secret/telemetry
  names. This static check does not replace the future spawn environment policy
  or consent review and cannot identify every secret in an arbitrary string.
- Alias consistency checks compare configured backend identity, pool and ceiling;
  equivalent host case/default ports/trailing slashes are normalized without I/O.
  No DNS alias discovery, physical capacity discovery or cross-process admission
  coordination is implied.
- Configuration input/output is bounded to 1 MiB. Parser errors expose a category,
  safe key when available and byte span, never the raw parser message or source
  excerpt. A future file loader supplies the separately sanitized file path.
- Zero animation FPS and zero eligible provider retries are valid. Automatic
  expiry, persisted YOLO, active routing and inherited shell stdin are rejected.
  Enabling telemetry/Laya or requesting an allow policy never grants permission.

## Dependencies and verification

Serde 1.0.229 (`std`, `derive`) and toml_edit 0.25.15 (`parse`, `display`, `serde`)
were selected from the existing local cache and resolved offline with Rust 1.98.
Their transitive versions and feature ceilings are recorded in Cargo.lock and
`scripts/check_dev_setup.py`. Core/TUI cannot acquire the TOML stack; normal and
build paths still reject forbidden Fluzo coupling. Workspace optional features
remain unsupported. The checker rejects unknown packages, sources, versions and
features rather than allowing arbitrary dependencies. This is a reviewed graph
policy, not proof that third-party build code is sandboxed.

Initial dependency preparation is separate from inference-independent checks:

```sh
cargo fetch --locked
```

That command needs registry access on a fresh machine; it was not needed locally.
Subsequent checks remain offline. The LSP fixture uses `cargo vendor --locked
--offline` to copy locked dependencies into its temporary directory and generates
only its source-replacement config, preserving an empty HOME/CARGO_HOME with no
inherited Cargo credentials, application settings or prior build artifacts.

```sh
cargo test -p fluzo-core --locked --offline
cargo test -p fluzo-runtime --locked --offline
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
cargo build --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_lsp.py
```

On 2026-09-25 the commands above passed for the uncommitted FND-02 working tree:
28 Rust tests, 24 Python tests, formatting, check, Clippy, build, graph/skills
checks and the isolated real LSP fixture. The Linux x86_64 filtered Cargo graph
also passed; the unchanged bootstrap help command still reports unavailable
execution. These results are local evidence, not a remote workflow result or
maintainer acceptance. Existing skill-migration changes and staging were preserved.

Tests cover typed defaults/descriptors, TOML round trips, every registered numeric
constraint, optional references, malformed/unknown input, safe diagnostics,
capacity alias/role conflicts, monetary price requirements and pure draft edits.
Graph regressions include transitive/build coupling, target-conditional forbidden
TOML dependencies, unreviewed sources/versions/features and inactive workspace
features. No tests contact configured endpoints or require live inference.

Implementation checks initially exposed TOML deserializer type mismatches, a
nested metadata macro delimiter error, Clippy findings and a bootstrap fixture
assuming all third-party features were empty. These were corrected and rerun;
failed attempts are not passing evidence. Runtime execution, save/apply behavior,
visual acceptance, remote CI and live inference remain unverified here.
