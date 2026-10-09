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

## Normal configuration (UI-03 S3)

S3 (#46, refinement R2) builds on main `d03b60d84e7dcd225348baf2fe8888cbd22640d9`
and approved fluzo-docs revision `ed08afab9e2e3ac22a9ef5ef89e32911fcda1850`,
PRD 26/29.2.2/30 and architecture A06/A07, 10.2/10.3. With valid selected
configuration, interactive `fluzo` opens a configuration-only shell, not the
synthetic demo or an execution runtime. Ctrl+P opens searchable Configuration
entries; All / advanced exposes every registry field. Ordinary setup offers F2
into this shell after confirmed creation. Explicit init remains setup-only.
Headless contracts and the explicit demo's isolation remain unchanged.

All categories use the shared centered dialog style, including Limits and Pools.
Help, replacement inputs, collection-name inputs and F4 advanced setup reuse the
same frame and title rather than switching to a separate full-screen form. See
TUI.md for layout, fallback profiles and pending human visual review.

The editor uses owned descriptors from configuration protocol 5. Types, defaults,
units, privacy, provenance, saved/draft/effective values and CLI locks are shown.
Fixed schema values have no editable alternative. Models/pools have validated
names and explicit add/remove operations; related changes validate together.
Unchanged redacted/sanitized projections are never used as replacement input.
Fields other than booleans and enumerated choices edit inline in the value column, with a cyan background and
visible cursor (underlined without color). Typing or pasting initially replaces
the current input; arrows allow editing it instead. Enter stages a valid value
and leaves editing; Esc discards the input. Empty edited input restores the shared
registry default, including nonempty list/map defaults. Enter without changing
input preserves the original, including redacted values. Private inputs start
blank and remain masked. Credential inputs are references, not credential
resolution. Lists use one escaped item per line; maps use escaped key=value entries.
Supported escapes are backslash, equals, n/r/t and whole-item `\\e` for an empty
item. Duplicate map keys are rejected; Shift+Enter adds an input line. Long and
multiline inputs scroll horizontally with the cursor and display escaped controls.
Save to disk and Apply remain separate explicit operations.

Settings lists use two aligned key/value columns without table borders. The
focused row has a continuous theme-accent background across its label and value.
Boolean fields display only true or false while navigating. Enter reveals inline
true/false choices in the value column, with cyan background and black text on
the chosen option. Only editing uses cyan; confirmation or cancellation returns
to the plain value. No-color editing retains explicit x markers. The full key
remains available in details when the label column clips it. No free-text boolean
input is required.
Up/Down selects a field; Enter focuses its choices, Left selects true and Right
selects false. A second Enter stages the choice; Esc cancels without changing the
previous draft. Paste, typing and arrows outside edit focus cannot change a
boolean. F4 advanced setup uses the same control. Full keys remain in setting
details; Save/Apply selection is separately marked `[save/apply]`. Fixed fields
such as `storage.auto_expire` show `[fixed]` and cannot enable unsupported behavior.

All enumerated choices, including theme and authorization mode, follow the same
inline selection contract as booleans. Enter reveals supported options;
Left/Right selects an option, Enter stages it and Esc cancels. The current value
alone is shown outside editing; typing, paste and deletion cannot alter a choice.
Options scroll within the value column to keep the selection visible. Fields
with fewer than two supported choices remain fixed. Advanced setup uses the same
selector. Cyan is reserved for the selected option during editing, with textual
markers without color; Save and Apply remain explicit.

Controls: type to search, Ctrl+U shows all fields, Tab/arrows select, Left/Right
step numbers outside editing, Enter edits, Space selects
Save/Apply keys, Ctrl+D restores the selected default in the draft, Ctrl+N adds a
model, Ctrl+P adds a pool and Delete stages collection removal. F1 explains all
controls. Ctrl+V validates, Ctrl+S saves selected future values, Ctrl+A applies
selected presentation values. Without explicit selection, Save uses unsaved keys;
Apply uses current edits plus previously validated, unlocked presentation/restart
keys awaiting application. Explicit selection can still request an unavailable or
CLI-locked operation and receives its typed rejection. Successful completion clears
only the corresponding pending scope and completed selection, not other drafts.
Ctrl+X twice discards drafts; Ctrl+R twice reloads/discards. Neither undoes prior
Save or Apply. Esc closes the view retaining drafts and composer/focus/scroll.
Closing during validation prevents the chained Save/Apply; dispatched effects
still reconcile by request identity and are never automatically replayed.

The runtime projects Save/Apply unavailability and remaining request capacity.
The normal host runs with `LocalWorkspace`, so Save replaces the existing `.fluzo`
with the user's edits. No CLI flag, confirmation or config data grants this
capability; the host wiring does. Session presentation Apply stays independent.
Operational Apply stays unavailable; restart state is pending only. CLI overrides
govern active settings, not the separately saved future defaults.

The normal host renders that state as a `Restart pending: <keys>` row in the
settings details area, or `Restart pending: none` when the list is empty. The row
is presentation only: it offers no restart action, and a presentation Apply for
an unrelated key never clears it. Restart application remains deferred to
REL-01 (#24).

The service retains 64 request records without eviction. Keystrokes remain local;
a Validate/Edit consumes one record, chained Save/Apply needs a second. Exhaustion
preserves the displayed draft and retained outcomes, rejects new mutation and
requires reconciliation followed by explicit application reopening. Reopening
loses unsaved in-memory drafts; it is not automatic recovery or retry. Input fields
are bounded to 8 KiB and edit requests retain existing count/byte limits. Oversized
atomic changes are rejected, not split. Private configuration values must not be
copied into diagnostics or test artifacts.

### S3 verification and parent evidence mapping

Focused commands:

```sh
cargo test -p fluzo-cli --test configuration --locked --offline
cargo test -p fluzo-tui --locked --offline configuration
cargo test -p fluzo-runtime --lib --locked --offline configuration
python3 -B -m unittest discover -s scripts -p 'test_configuration.py'
python3 -B -m unittest discover -s scripts -p 'test_setup.py'
```

The CLI-crate integration harness composes the actual TUI editor and shared worker
with private real files, independently parses saved data and verifies comments,
redacted values, collection transactions, Save/Apply distinction, conflicts,
operational rejection, CreateOnly refusal and record exhaustion. TUI unit fixtures
verify typed inputs, registry coverage, late completion, failure/uncertainty,
redaction, bounded paste and representative buffer sizes. These unit fixtures do
not prove filesystem durability. Three actual-binary PTYs verify ordinary entry,
setup-to-settings handoff, presentation Apply, CLI locks, preserved composer/file,
resize, signal restoration and a registered endpoint receiving zero requests.
A silent endpoint is not evidence of universal network isolation.

| Original #8 criterion | Revision-linked evidence and remaining gate |
| --- | --- |
| Missing config opens setup | Accepted S2 (#45), merges `196140b`/`d03b60d`; existing setup PTYs plus S3 explicit F2 handoff regression |
| Valid config skips setup | Accepted S2 discovery; S3 ordinary shell PTYs with unavailable credentials/provider and devmenu disabled |
| Save/reload, defaults, conflicts and CLI precedence without models | Accepted S1 (#44), merge `83c42b9`; S2 setup evidence; S3 real editor/worker tests, buffers and PTYs under the same R2 write boundary |
| Linked evidence and documentation | This section, TUI.md, APPLICATION.md and the scoped S3 delivery; final reviewed revision and human acceptance still required |

Local validation of the S3 working tree based on `37a5077` (content-identical to
integrated `d03b60d`) passed 193 Rust cases and 66 Python cases, including three
new settings PTYs and eleven setup PTYs. Format/check/Clippy/build, development
boundaries, isolated LSP and the standalone private-network HTTP profile passed.
Commands are the AGENTS.md suite and focused commands above. The final Python
suite completed in 76.234 seconds; timing is not a performance claim.
S3 is uncommitted implementation evidence until delivery records identify its
revision. Initial shell integration exposed incorrect Cursor construction and
helper names; compiler errors were corrected. The pre-S3 valid-startup PTY expected
the old setup-only status and exit key; it was updated to verify the new normal
shell and preserve the no-write assertion. Initial Clippy findings were corrected
without suppressions. No before-change behavioral failure is claimed for the new
editor. Human Ghostty/Alacritty review, parent acceptance and remote CI remain
separate pending gates; live inference is not_run. C3 wiring, production consent,
agent execution and broader visual/performance acceptance are not implemented.

### S3 review corrections

The review of integrated #51 (`724fa37`, content-identical to `0819ec7`) found
stale operation selection and effective-only collection identity defects. Save and
Apply now retain separate pending key sets: saving a removal cannot resubmit it on
later saves, and previously saved operational or CLI-locked values do not pollute
subsequent default presentation Apply. Current intentional edits are still checked
by runtime; unsupported/locked edits are not silently applied or discarded.
Partial success clears only completed keys, while failed operations retain drafts
and explicit selection. Save-then-Apply and Apply-then-Save remain independent.

Collection creation checks current draft existence and local additions instead of
all projected descriptors. A removed model/pool may remain in the effective
snapshot until later runtime integration, but that does not prevent recreating its
name in future configuration. Effective values remain visible and unchanged;
existing draft collections and duplicate local additions are still rejected.

Three new regression tests compiled and failed on the intended assertions before
correction: subsequent Save returned InvalidRequest, presentation Apply retained
the default theme after Unavailable, and collection recreation was rejected.
They now pass, with an additional partial-completion/failure preservation test.
Existing actual-binary PTYs now verify Apply after CLI-lock rejection, retention of
the locked draft and collection recreation after draft removal. An initial PTY
extension reused an already-visible completion marker and sent input while pending;
explicit intermediate screen transitions fixed the harness without timing sleeps.

Local correction validation passes 197 Rust cases and 66 Python cases, plus
fmt/check/Clippy/build, development boundaries, isolated LSP and private-network
HTTP checks. Commands remain the AGENTS.md suite and S3 focused checks above.
No new dependencies, protocol changes or write capabilities are introduced.
Human visual/parent acceptance and correction publication remain separate; live
inference is not_run. These fixes do not accept or close #46/#8.

### S3 visual color correction

Human review of `1705ddb` in Ghostty found a gray normal shell and a flattened
wordmark. Normal startup left the shell's truecolor capability disabled, unlike
the explicit demo. It now uses the same TERM/COLORTERM detection, preserving
NO_COLOR and limited-terminal behavior without changing the artwork or FPS policy.
This is a scoped #46 correction under the S3 design sections cited above.

The actual-binary settings PTY regression failed before the fix for Ghostty,
COLORTERM=truecolor and COLORTERM=24bit because RGB output was absent. All now
pass, along with non-RGB, NO_COLOR and Linux-console profiles and the setup F2
handoff. These checks retain file preservation, CLI-lock/Apply, resize, terminal
restoration and zero requests to the fixture listener. The workspace's 197 Rust
cases, 67 Python cases, fmt, Clippy and dependency-boundary checks passed; the
added RGB setup-handoff subcase also passed in the focused settings suite.
Corrected human appearance remains pending review; no visual acceptance or issue
closure is implied. Live inference is not_run.

### Requested startup and setup revisions

The S3 human review requested screen defaults instead of setup questions, a
conservative startup option and an enabled MVP developer-menu default. The setup
form no longer submits TUI edits; future screen preferences come from the registry.
The full shared relief wordmark and RGB palette now appear in setup as well as the
normal shell, with ASCII and limited-color fallbacks. Presentation settings remain
available through ordinary Configuration and Developer menu > Presentation settings.
The latter is a typed-editor entry, not the synthetic demo or complete C3 delivery.

`fluzo --safe-screen-settings` selects ASCII, no color, FPS 0, reduced motion,
no render diagnostics and no desktop notifications for this invocation. It takes
precedence over conflicting valid visual flags regardless of order. Emergency
values are CLI-locked and are not written by setup; files are not reset or repaired.
Reopen without the option to use normal presentation. Existing-file Save still
requires the supported coordinated host; this option does not bypass that policy.
The shared `tui.dev_menu` default is true for the MVP; explicit configuration and
CLI disable remain respected. The review launchers must not disable it implicitly.
These requested changes are local review work, not a published design revision.

Repeated PTY resize checks preserve 80x24, 120x40 and 160x50 dimensions and reject
window-resize escape sequences. Inspection of Ratatui 0.29 confirms `resize` only
updates buffers and clears the viewport. The reported Ghostty/Wayland window-size
reset has not been reproduced or fixed; compositor/emulator behavior needs an
actual-window reproduction on Wayland. A disposable, owned Ghostty 1.3.1 X11
window retained three requested pixel sizes (960x600, 1280x800, 800x500) over
bounded two-second observation intervals. That does not prove Wayland behavior.
No terminal preferences or KWin rules were changed.

At the startup-review checkpoint, discovery remained unimplemented. The subsequent
user-authorized local/LAN wizard is described below. GitHub provider integration
remains deferred to a later user story. Neither change establishes S3 acceptance.

### Explicit local/LAN model wizard

F3 opens Add model from setup or ordinary configuration. The ordinary palette also
contains Add model > Local / LAN. Centered dialogs reuse the shell's rounded frame,
gradient title, theme and retained background. Steps: Local or Frontier (unavailable
until its separate story), LM Studio/Ollama/OpenAI Compatible, Authorization yes/no,
optional environment reference, URL, multiple model selection, alias prefix/review.
Leaving the URL using Tab, Enter or a mouse click outside the field queries once;
typing, paste, resize and operating-system window focus never query. Esc goes back
without querying and cancels pending work. All providers currently use /v1/models.

This initial adapter accepts HTTP with a localhost, literal IPv4/IPv6 or DNS host,
an optional port and optional /v1 suffix. The worker resolves DNS names itself and
keeps the original authority in the Host header, so virtual-host and dynamic-DNS
catalogs route correctly. The resolved address class is not restricted: locality is
the operator's responsibility. HTTPS, custom paths, redirects, userinfo,
query/fragment and percent escapes are rejected without downgrade. Optional
Authorization uses a named environment variable
containing the complete header value (for example Bearer plus a token). The worker
resolves it only for the user-triggered request; invalid/missing values fail before
connection. The UI, DTOs and saved configuration contain only the variable name.
Header values are bounded to 4096 bytes, reject control characters and are marked
sensitive. HTTP is unencrypted: use only trusted local/LAN destinations and scoped
credentials. No inherited proxy, LAN scan, redirect, automatic retry or inference.

The multi-select list shows ID, maximum context, output tokens and slots when the
catalog reports max_context_length/context_length, max_output_tokens and slots.
Missing or invalid values display ?. These are unverified reported hints, not
capability discovery. No native provider enrichment is claimed. Space toggles a
model; selection survives filtering. A single model keeps the chosen alias;
multiple models receive numbered aliases. All are staged atomically, or none on
conflict/capacity failure. Registry limits and one-slot admission remain defaults
until reviewed separately; reported slots are not automatically adopted.

Discovery uses a separate owned core protocol and runtime worker. One request is
active, the total deadline is five seconds, HTTP headers are bounded to 16 KiB,
body to 1 MiB, catalog to 256 items and each model ID to 256 bytes. Status is polled
without network effects. Esc during a query requests cancellation; late results do
not reopen the wizard or stage data. The worker retains its pending slot until
completion, supports at most 64 submissions, and is cancelled/joined on shutdown.
Only the latest result is retained; older IDs become Unknown and cannot replay.

Confirming creates each selected model and an independent single-slot pool with
its alias in the local draft, never overwriting an existing alias/pool. If used,
the Authorization variable name is saved as auth=env/api_key_env; its value is never
persisted. This is a catalog-header reference, not an implemented inference adapter. Setup reserves
coder/shadow/local for its manual fields and selects the first discovered alias
when no manual coder is configured. Ordinary settings do not change agent.model;
select it explicitly there. Save remains separate: setup still requires review
and confirmation. The wizard grants no permission for later inference and sends
no request beyond the catalog query the user triggered.

Focused checks: `cargo test -p fluzo-runtime --locked --offline model_discovery`,
`cargo test -p fluzo-tui --locked --offline model_wizard`, and
`python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'`. The PTY
composes actual CLI, discovery and configuration workers with a registered fake
catalog and independently verifies saved TOML. Runtime tests cover redirects,
429 without retry, cancellation, timeout, bounds and malformed model IDs.
Native discovery is not a live agent test or human visual acceptance.

The implementation is original Rust; Crush revision 76cc5c5 was inspected only as
an interaction reference under its FSL-1.1-MIT license. No source or artwork was
copied. Cached, already pinned Hyper/Tokio/JSON packages are now allowed on runtime
production paths and transitively CLI; core/TUI HTTP and direct build edges remain
forbidden. No dependency was downloaded. This is a user-requested extension beyond
the original offline S3 baseline, not a published issue/design acceptance.

Validation of the uncommitted wizard on base `2520f22` passed 207 Rust cases,
71 Python cases, workspace format/check/build/Clippy and dependency-boundary checks.
The oversized-response fixture initially replied before reading the request and
failed with Connection/BrokenPipe; synchronizing on request headers fixed the
fixture. The graph tests were updated to admit only the new runtime HTTP paths,
while preserving TUI/core prohibitions and rejecting direct build dependencies.
A separate manual catalog GET was also made to a local endpoint during development;
that was not isolated simulator evidence and did not run inference. Its returned
model identifiers are deliberately not recorded here. No human visual acceptance
of the new wizard, live inference or remote publication is claimed.

The subsequent guided-dialog revision passed 209 Rust cases and 72 Python cases,
plus format/check/build/Clippy and boundary checks. Its PTYs exercise URL Tab
queries, optional Authorization from a synthetic environment variable, multiple
selections and independent verification that only the reference reaches TOML.
Unit checks cover protocol-1 rejection, missing/invalid header references before
connection, unknown versus reported metadata, late-result cancellation, atomic
multi-model rejection and shared dialog frames at 60x16 through 160x50. A clipped
minimum-size footer was found by the buffer test and shortened without removing
its back/cancel control. Frontier remains explicitly unavailable; no new live
catalog query or inference was made for this revision. Human review is pending.

The resulting local working tree passed 200 Rust cases and 69 Python cases,
including safe-screen file preservation, unchanged setup screen defaults,
MVP developer-entry navigation, setup relief buffers and repeated-resize PTYs.
`cargo fmt --all -- --check`, workspace check/build/Clippy with locked offline
resolution and `python3 -B scripts/check_dev_setup.py` passed. Python unittest
discovery includes the existing simulator and LSP fixtures; no live inference ran.
The old default-disabled developer-menu assertion initially failed after the
requested default change; it now verifies default-enabled and explicit-disabled
behavior separately. No fixture assertion was removed to hide the resize report.

## Local writes and outside changes

The operator decision for this workspace is that Fluzo owns `.fluzo`: the tool
may replace the file whenever the user saves, and outside edits are information
to surface rather than a write barrier. `fluzo-cli` starts the configuration
service with `WritePolicy::LocalWorkspace`, so no Save path returns `ReadOnly`
for an existing file. Replacement stays atomic: new bytes go to a temporary file
in the same directory and move into place with a directory rename, so a crash
cannot leave a truncated configuration.

No backup is created. The workspace relies on Git to recover a bad save. The
existing backup machinery stays available to hosts that ask for it; the normal
Save path passes `backup: false`.

Because the file can now change underneath the writer, the worker polls it. Every
two seconds it stats the target and compares a fingerprint (device, inode,
length, mtime, ctime, mode, link count) against the last identity this writer
owned. A mismatch is classified as `Appeared`, `Modified` or `Removed`, recorded
on the snapshot as `external_change`, and counted by `external_sequence` so a
notice the user already acknowledged resurfaces when the file moves again. A
failed probe is not evidence of an edit: the last known state is kept and the
next poll retries. Detection adds no dependency; `notify`/`inotify` stay out of
the lockfile.

The settings view shows a banner naming the kind of change and offers two
answers. `Ctrl+R` reloads, adopting the outside bytes and discarding the local
draft. `Ctrl+K` keeps our draft and hides the banner. Neither touches the file.

Save is deliberately the "our edits win" path. When a Save arrives while an
outside change is pending, the service first rebases onto the file actually on
disk: the outside bytes become the saved baseline, then only the keys the user
touched are applied on top. Unrelated keys the outside edit introduced survive,
and the user's intent is not lost to a conflict error. For this to work the
polling check must not bump the snapshot version; bumping it would make the view's
own stale-draft guard reject the Save instead of performing it.

Focused checks: `cargo test -p fluzo-runtime --lib --locked --offline
configuration` covers classification without version changes, the rebase merge
that preserves an unrelated outside key, recreation of a file removed outside
Fluzo, reload adoption and a real two-second worker poll. `cargo test -p
fluzo-tui --locked --offline configuration` covers banner rendering,
acknowledgement, resurfacing on a later change and the pending-write guard.
The PTY case `test_outside_change_banner_offers_keep_ours_then_reload` in
`scripts/test_configuration.py` drives the real binary through banner,
keep-ours, Save merge and reload.

This relaxes the read-only-for-replacement boundary previously described for
ordinary hosts. Because it changes a default with security relevance, the
matching amendment is recorded in the documentation baseline instead of
assumed: [PRD 26.1](https://github.com/fluzo-labs/fluzo-docs/blob/3c7fc8e1f6934922ad66fe3eda8123854620839b/PRD.md)
and architecture 10.2 at fluzo-docs revision
`3c7fc8e1f6934922ad66fe3eda8123854620839b`. The behaviour described above
implements accepted policy, not a pending proposal.

## Friendly repository creation

The default setup surface is a centered dialog, not a registry form. It explains
missing configuration, offers Create, Add models (F3) and Not now (Esc), and uses
recommended defaults. Create first prepares a candidate; a separate confirmation
creates the file. The confirmation shows human-readable summaries and the selected
path, not variable names. F4 retains explicit access to advanced fields and the
full technical review. No values, writer capabilities or validation rules change.
Loading has its own title; the welcome action appears only after discovery.
After successful creation, ordinary Enter/F2 opens Settings; init remains separate.
Declining, pasting and reopening never authorize creation. Conflicts and uncertain
outcomes still preserve their explicit diagnostics, without automatic replay.

## Offline setup (UI-03 S2)

The following describes the S2 baseline; the S3 review revisions above remove
screen questions while retaining its offline and write-capability boundaries.

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

For replacement, the host must explicitly select `LocalWorkspace` (renamed from
`CoordinatedLocalWriters`) for a trusted, stable workspace where all concurrent
writers honor the same parent-directory exclusive lock. Use `ReadOnly` or
`CreateOnly` otherwise. This is an integration precondition,
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

The Developer Menu is enabled by default during the MVP, as requested during
S3 human review. Open it through the command palette; an explicit `--no-dev-menu`
still wins over `--dev-menu`. Ordinary startup offers Developer menu > Presentation
settings through the shared typed editor, without synthetic runtime actions.
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
