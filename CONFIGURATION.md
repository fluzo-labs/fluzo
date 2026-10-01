# Typed configuration foundation

This implements the library scope of [FND-02 (#2)](https://github.com/fluzo-labs/fluzo/issues/2),
using [PRD v0.2 sections 26, 27 and 43](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/PRD.md)
and [architecture A06, section 10](https://github.com/fluzo-labs/fluzo-docs/blob/60c5b0732fb710cdf705476cee8d9156a5ecd971/ARCHITECTURE.md).
FND-02 alone does not implement a wizard, configuration CLI, application port,
file saving, active-task application, credentials service, provider or agent
execution. UI-03 S1 adds the shared service described below; setup and normal
configuration views remain separate work.

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

## Offline setup (UI-03 S2)

S2 (#45, refinement R2) uses the write-capability decision in fluzo-docs #11,
revision `ed08afab9e2e3ac22a9ef5ef89e32911fcda1850`, PRD 26.1 and architecture
10.2/10.3. Run `fluzo` in the selected workspace or `fluzo init` to open setup;
`--config` selects a relative or workspace-contained absolute path. A valid file
skips the form even when its provider or credential reference is unavailable.
The startup view reports runtime unavailability honestly and offers no execution.
The isolated `demo` commands retain their no-config-read/write contract.

The form uses shared field descriptors/defaults. Basic fields cover an optional
`coder` model, environment/named credential references, context/output capacity,
model/pool slots and presentation preferences. Advanced fields cover optional
`shadow` Laya preferences, storage quotas and OTLP preferences. These fixed setup
aliases are not a general collection editor. Blank endpoint/model fields allow
saving a valid configuration that is not execution-ready. No probe, inference,
credential resolution or consent grant occurs, including when optional services
are enabled. Connection tests are explicitly unavailable.

Enter opens the form or edits a selected value; Tab/arrows navigate. Ctrl+U clears
an input, Enter finishes editing, Ctrl+A toggles advanced fields. Ctrl+S first
requests a validated redacted summary; a second explicit Ctrl+S confirms creation.
PageUp/PageDown scroll the summary and target. Esc returns from review without
saving, or exits the form. Ctrl+C/Ctrl+Q exit. Inputs are bounded to 2,048 bytes
per field; paste cannot confirm. Sensitive inputs are masked, never echoed into
summaries. CLI presentation overrides stay authoritative and visible; they do
not overwrite the separately saved future defaults.

Normal CLI hosts use `CreateOnly`: the same runtime adapter publishes exclusively
without replacing a destination that appeared concurrently. After creation this
capability does not allow replacement. Existing invalid/inaccessible files remain
untouched; `fluzo init` reports replacement unavailable outside a controlled host.
The existing supported Linux/path/filesystem restrictions remain in effect.

Configuration protocol 3 adds PrepareSetup/ConfirmSetup, an optional redacted
preview and retained backup reference. Preparation validates a fresh candidate
independently of an invalid original, reads the bounded original bytes and binds
confirmation to the service version. It creates no files. Any intervening action
invalidates preparation; stale confirmation fails. Confirmation consumes the
preparation before attempting the effect, so failure cannot authorize replay.
Normal Save still rejects invalid input. Save/setup creation do not apply active
settings or change CLI overrides. Reload is required after uncertain completion.

Only explicitly coordinated hosts may replace an existing file. Confirmation
creates an exclusive independent `.fluzo-backup-<pid>-<serial>` copy, mode 0600,
of its exact bytes, including invalid UTF-8. Content is read back and verified,
and file/directory synchronization precedes replacement. Backup failure blocks
replacement; any created backup is retained and exposed by snapshot for recovery.
The original is rechecked before replacement. Originals above the existing 1 MiB
limit, unsupported paths and inaccessible originals are rejected. No automatic
backup removal, restoration, migration or retry is performed. A retained backup
reference alone is not proof that a failed backup completed successfully.

Headless startup never opens a wizard. It returns failure with a JSON diagnostic
on stderr and empty stdout: `configuration_required`, `configuration_invalid`,
`configuration_inaccessible`, `replacement_unavailable`, or `runtime_unavailable`.
Startup observation has a five-second wait bound; this does not interrupt blocked
OS I/O. Closing after dispatch does not establish cancellation or undo a write.

Focused checks: `cargo test -p fluzo-runtime --lib --locked --offline setup`,
`cargo test -p fluzo-tui --locked --offline setup`, and
`python3 -B -m unittest discover -s scripts -p 'test_setup.py'`.
Tests cover real exclusive creation, independent backup bytes, backup faults and
collisions, stale candidate/file confirmation, uncertainty, CLI separation,
headless discovery, isolated PTY save/cancel/conflict/resize/restoration and a
registered local endpoint receiving no requests. No application live inference
or human visual acceptance is implied. New APIs initially caused exhaustive-match
and snapshot-constructor compilation failures during migration. Clippy required
moving a test module; a PTY resize case exposed truncated minimum-size guidance,
then required a wrapped-line assertion. These failures were corrected without
weakening existing tests. No before-change behavioral failure is claimed for
previously absent setup APIs.

Local validation of the uncommitted S2 changes based on `1f80956` passed 174
workspace Rust cases (24 configuration cases, including 5 setup cases, and 90
TUI cases), 62 Python cases (including 10 setup cases), fmt/check/Clippy/build,
dependency/skill checks, isolated LSP and the standalone private-network HTTP
profile. Commands are the AGENTS.md suite and focused commands above. No remote
CI run or manual setup visual acceptance is implied. Scope remains S2 only;
S3/C3 and production runtime execution are not implemented by this change.

### S2 review corrections

The final review of #49 found three setup regressions. Explicit pool preferences
without a model were omitted; setup now creates the selected pool when its fields
change, while untouched defaults still produce no model/pool entries. Summary
formatting now covers fractions, lists and maps as well as scalar values, without
changing redaction. Leaving a scrolled summary resets the viewport when the
preview disappears, preserving selected field and draft on Cancel or Reload.

Three focused tests compiled and failed on the intended assertions before the
fixes, then passed with `cargo test -p fluzo-tui --locked --offline review_regression`.
An additional private-PTY scenario checks visible pool edits, fractional summary
values, return navigation and independent on-disk preservation without models.
The correction tree based on `e106ff8` passes 177 workspace Rust cases and 63
Python cases, including 11 setup cases, plus fmt/check/Clippy/build, dependency
checks, isolated LSP and the standalone private-network HTTP profile. Commands
remain the AGENTS.md suite. These are local results, not remote CI or evidence
of a new human Ghostty/Alacritty visual session. Protocol 3, defaults, write
capabilities and the accepted demo appearance are unchanged.

## Shared configuration service (UI-03 S1)

[#44](https://github.com/fluzo-labs/fluzo/issues/44), under UI-03 #8, adds
`fluzo_runtime::configuration::ConfigurationService` implementing core's separate
`ConfigurationPort`. Design: PRD 26/29.2.2/30 and architecture A06/A07 sections
5.1/10, with approved D0 revision `53b345e9f1782beef839b42b4c0de4773ed371df`.
This is a reusable library; S2 now uses it for setup. Normal settings views and
C3 menu wiring remain outside this increment.
The existing CLI demo remains isolated and does not load or save user settings.

The composition root supplies an absolute workspace path, optional relative or
workspace-contained absolute config path, typed CLI overrides and a write policy.
No HOME/global configuration or credentials are discovered. Startup happens on a
dedicated worker. Missing files use shared defaults; invalid/inaccessible files
produce explicit discovery/problem state and cannot be overwritten by Save.
Explicit Reload discards the current draft on a valid read, updates saved values,
and never silently replaces active effective settings. Invalid reload preserves
the prior valid values while exposing the new problem.

`config::update_batch` edits schema-defined fields and model/pool collections in
one candidate document, then validates the complete result. This avoids invalid
intermediate model/pool references. Add/remove collection operations are explicit;
Save selects leaf keys or whole `models.<id>` / `capacity_pools.<id>` entries.
Selecting an incomplete related change fails validation without a partial save.
An empty Save on a missing file creates the generated non-secret defaults.

Save changes only future configuration. Apply validates selected presentation
settings against the effective snapshot; operational application returns
`Unavailable`. CLI overrides remain effective and locked for Apply, but future
file values may be saved separately. Cancel restores the saved draft without
undoing prior Save or Apply. Restart-rule selections are recorded as pending,
not activated. No task, budget, deadline, grant or remote transport is owned by
this service. Production operational enforcement is not certified by its fixtures.

### Filesystem and concurrency contract

The initial adapter supports Linux x86_64, a usable `/proc/self/fd`, and local
filesystems with file synchronization, directory synchronization, hard links,
atomic same-directory rename and directory advisory locking. No dependency or
unsafe code is added. Paths are traversed component-by-component through held
directory handles with no-follow opens; parent traversal, directory symlinks,
non-regular targets and multiply-linked configuration files are rejected.
Parent identity and target content/metadata are checked before saving.

For replacement, the host must explicitly select `CoordinatedLocalWriters` only for a trusted,
stable workspace where all concurrent writers honor the same parent-directory
exclusive lock. Use `ReadOnly` or S2's exclusive `CreateOnly` otherwise. This is an integration precondition,
not user consent, an OS sandbox or protection against arbitrary editors or a
hostile process that ignores locks. The adapter cannot detect that all external
writers cooperate or certify a network filesystem. No caller may silently opt
into this policy for an unverified production workspace. S2/S3 integration must
retain this limitation or obtain a reviewed stronger adapter before offering
writes outside the supported environment.

Under that contract, a nonblocking directory lock serializes check-and-replace
across cooperating processes without a removable lock-file race. The original
observation includes exact bounded content and inode/device/timestamp metadata;
external changes observed before commit require Reload/reconciliation. A final
version check plus rename alone is not claimed to prevent uncooperative races.
Creation uses exclusive temporary creation and a no-replace hard link, so an
unexpected newly created target is not overwritten. Replacement uses atomic
rename. Temporary files are mode 0600, flushed before commit; the parent is
synchronized afterwards. Existing broad access permissions are not propagated.

Precommit failure preserves the previous file. Failure after replacement or
parent synchronization is `Uncertain`, blocks further Save/Apply, and requires
explicit Reload; it never claims rollback. A process crash can leave a private
temporary file. There is no automatic orphan deletion or operation replay.
Reopening reads actual disk state; in-memory request IDs do not promise durable
exactly-once execution across process restarts. Network-filesystem hangs and
power-loss durability are not established by local injected-failure tests.

### Evidence and limitations

Focused command: `cargo test -p fluzo-runtime --lib --locked --offline configuration`.
The S1 suite covers actual creation/replacement/reload, selected saves, collection
transactions, CLI precedence, invalid documents, external creation/removal/inode
replacement, locks across a child process, symlink/hardlink refusal, injected
pre/post-replacement and synchronization failures, queue limits, replay and
redacted projections. Tests use private temporary roots and bounded process
handshakes, not providers or sleeps as scheduling evidence.

The initial batch-helper compilation failed due to a retained String return
where unit was required; this was corrected before the service tests ran. It was
not a reproduced behavior regression. New service behavior had no previous
implementation to test. Synthetic operational assertions establish isolation,
not production capacity draining, audit persistence or consent enforcement.
Live inference is not run. No visual appearance changes are part of S1.

Local validation on the uncommitted S1 implementation based on `8d709d9`:
162 Rust test cases passed (15 configuration cases, including one child-process
helper), 52 Python cases passed, and format/check/Clippy/build, dependency/skill
checks, isolated LSP and the private-network HTTP profile passed. Commands are
the existing AGENTS.md validation list. No remote CI or human acceptance is
implied. Clippy initially rejected an unnecessary borrow after path hardening;
it was fixed without lint suppression. A restart test initially expected a
pending flag for an unchanged fixed-value setting; it now verifies no false
pending state and preservation of a synthetic existing pending state. Current
restart-class settings have no selectable alternative supported by the registry;
this is not evidence of a production restart implementation.

### S1 review corrections

The post-merge review of #47 found three regressions, reproduced before correction
by `cargo test -p fluzo-runtime --lib --locked --offline review_regression`:

- Apply expanded the entire effective configuration into TOML and could reject
  a valid sparse file below 1 MiB. It now adapts only the small presentation
  subset, assigns it to a typed candidate and revalidates the complete candidate
  before replacement. Unrelated model/pool data and file bytes remain unchanged.
  The regression uses 9,000 pools in a valid 232,909-byte file.
- Invalid/inaccessible Reload could change the origins of retained valid values.
  Provenance now uses the last valid saved source and its file-presence flag,
  independently of the latest observation/problem. Tests retain saved, draft and
  effective values/origins for repository and explicit-file selections.
- Configuration error adaptation discarded categories and positions. Protocol 2
  carries the shared diagnostic code, safe key, optional byte range and validation
  errors without source values. Invalid encoding is distinct from an I/O failure;
  public-port and serialization tests cover diagnostics and old-protocol rejection.

The three original regressions failed by assertion before their fixes and now
pass. Intermediate compilation failures while migrating error fields and a
private-helper call were corrected; they are not behavioral test evidence.
These changes do not alter filesystem coordination requirements, permissions,
defaults or the selected interface. See APPLICATION.md for protocol compatibility.

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
