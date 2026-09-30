# Interactive visual prototype

UI-01 ([#6](https://github.com/fluzo-labs/fluzo/issues/6)) covers the initial
terminal shell, multiline composer and searchable command palette. References:
PRD 29/30/31 and architecture A07, section 12, at baseline
`60c5b0732fb710cdf705476cee8d9156a5ecd971`.

## C2 visual notification preview

This user-authorized integration on `feature/c2-notification-preview` builds on
PR #41 and supersedes the historical no-visual-integration notes below. It does
not redesign the selected logo, palettes, sidebar, composer or conversation layout.

Start `cargo run --locked --offline -p fluzo-cli -- demo --interactive --dev-menu`.
Open Developer Menu through Ctrl+P, filter `notification preview`, then Enter.
Opening enters an empty, clearly synthetic preview; it does not generate notices
or send anything outside the terminal UI. Composer focus, text and scroll remain
unchanged. While the preview is active:

| Key | Action |
| --- | --- |
| Alt+N | Add a synthetic notice, cycling INFO/SUCCESS/WARNING/ERROR/APPROVAL |
| Alt+B | Generate a 40-event burst to exercise bounded overflow |
| Alt+R | Replay the latest event without extending its deadline |
| Alt+D | Dismiss the oldest retained notice |
| Ctrl+A | Apply pending preferences for this session and close preview |
| Ctrl+R | Preview defaults for unlocked Developer Menu preferences |
| Esc | Close, clear synthetic notices and revert unapplied preferences |

Notices use the same rounded outer frame as existing dialogs (with the same ASCII
fallback). ERROR borders and close buttons use the theme's red error color;
WARNING uses its yellow warning color. Other severities retain the accent border.
NO_COLOR retains the frame and explicit severity labels without colors.
Each occupies five rows: three content rows plus
top/bottom borders, at the top-right of the conversation, with existing semantic
styles and explicit `[DEMO severity]` labels. Capacity includes the frame height.
Each top border has a clickable `x` with a three-cell target; left-button press
closes that notice only. Clicking the notice body, dragging, releasing or using
other buttons does not activate the conversation underneath or change focus.
Alt+D remains the keyboard alternative. They overlay only that
conversation region, never reflow it or cover the composer, footer, sidebar or
wordmark. Available slots are computed from conversation height; unsupported small
viewports and open dialogs yield zero. No slide, flash or extra animation is added.
Expiry runs on monotonic elapsed time even at FPS 0 and while notices are hidden.
Cached animation redraws repaint notices after activity indicators. All examples
use static synthetic text, not composer content or task output.

`tui.notifications.desktop_enabled` joins the reversible settings list. Ghostty
capability is required to edit it; invocation overrides remain visibly locked.
Only explicitly applied settings reach the desktop host. Disabling cancels pending
tests and re-enabling does not replay them. Ctrl+T and the existing text editor
remain separate explicit external-test actions; they are not triggered by internal
preview actions. Existing natural synthetic completion delivery still requires
applied opt-in and known blur. No native desktop backend or config writer is added.

### Crush notification research

Read-only source inspection of `charmbracelet/crush` revision
`2fbaa90b55ec711f713c35ed55602eeb789e6048` found two distinct paths:

- `internal/ui/model/status.go` stores one `InfoMsg`, draws its severity indicator
  and message over status-bar help, and uses a five-second default TTL. In
  `internal/ui/model/ui.go:1507-1532`, InfoMsg schedules a timer and ClearStatusMsg
  clears it. This is not the top-right multi-notice stack proposed for Fluzo.
- `internal/ui/model/ui.go:678-762` selects a desktop backend and gates delivery on
  focus-report capability, observed blur and non-disabled configuration. Auto
  selects OSC for SSH/macOS and native delivery for supported local environments;
  explicit choices include auto/native/osc/bell/disabled. The backend refreshes on
  capability/configuration changes. Agent completion, permission and question
  events request notifications from the UI, rather than rendering code sending them.
- `internal/ui/notification/osc.go` probes OSC 99 support and falls back to OSC 777.
  `native.go` delegates to a platform notifier. The notification-style picker in
  `internal/ui/dialog/notifications.go` persists its selection; Fluzo does not copy
  that global persistence, add backends or relax its explicit opt-in policy.

The repository reference is [Crush](https://github.com/charmbracelet/crush).
No upstream source was copied, built or executed, and no user terminal notification
was sent for this research. The chosen stack follows Fluzo's approved D0/C2 contract,
not a claim of pixel-for-pixel Crush notification behavior.

The integration adds deterministic buffer/input regressions for no implicit events,
deduplication, overflow, dismissal, expiry, zero capacity, preserved user state,
CLI/capability gating and full/cached rendering at 60x16/80x24/120x40/160x50 with
truecolor, ANSI and NO_COLOR/ASCII. A private-PTY scenario checks keyboard actions,
resize, FPS-0 expiry, preserved draft, restoration and zero external OSC output.
An initial PTY assertion used the wrong composer row and was corrected; Clippy
also caught two local style issues. Human visual refinement and complete UI-04
runtime/permission acceptance remain pending. No issue closure is implied.

Local verification of this uncommitted integration based on `81cb61b` passed:
144 Rust tests (84 TUI), 48 Python tests (nine terminal-suite cases), workspace
format/check/build/Clippy with warnings denied, dependency/skill checks and the
isolated offline LSP fixture. Commands are the full validation list in the C2 logic
section below. Remote CI and live inference were not run for this branch.

## UI-02 stabilization contract

This local increment stabilizes the existing visual demo for
[UI-02 (#7)](https://github.com/fluzo-labs/fluzo/issues/7), against PRD 29.1,
29.2.1 and 29.2.2 and architecture A06/A07 at the baseline above. It does not
complete or close the parent. GitHub remains the backlog authority; the scope
boundaries below describe implementation ownership, not a published issue split.
This section supersedes conflicting behavior or completion claims in the
chronological iteration notes below. Their test counts describe those older
working states, not the current tree.

### High-contrast fire palette

The high-contrast identity now uses a red/orange/yellow gradient on black,
including both wordmarks, the top activity bar, inline loading wave and label,
and model metric bars. Truecolor terminals retain the relief shading instead
of forcing the old basic-color artwork. Limited-color terminals use warm ANSI
colors; NO_COLOR and reduced-motion behavior remain unchanged. Success, waiting,
error and cancellation retain their semantic state tints and independent labels.
The default cyan/blue/violet theme is unchanged. Buffer tests cover the fire
palette, cached/full redraw equality and frozen motion in compact/wide layouts;
actual contrast still requires human review.

### Developer-menu review follow-up

The maintainer confirmed wordmark legibility, sizing, developer-menu behavior,
notification arrival and the fire palette during the Ghostty review. Full
cross-terminal contrast and runtime acceptance remain separate pending checks.
The developer menu now shows a selectable list with current values, plus the
selected entry's metadata. Up/Down and Tab/Shift+Tab navigate, Home/End jump, Left/Right edit,
Ctrl+U clears the search, and held arrow keys repeat. Typing still filters settings;
clear the filter to navigate the full list. Ctrl+A applies session preferences,
Ctrl+R previews defaults and Esc reverts. In Theme preview, Reset affects only
`tui.theme`; FPS, reduced motion and diagnostics remain unchanged. In the
Developer Menu, Reset affects all four editable visual preferences, even when
search filters the list. Both actions preserve CLI locks and remain reversible
until Apply. The selected entry's CLI lock is shown immediately and never carried
to another entry. No persistence is added.

### C1 reversible-control corrections

[C1 (#36)](https://github.com/fluzo-labs/fluzo/issues/36) follows the approved D0
catalog in `fluzo-docs` revision `53b345e9f1782beef839b42b4c0de4773ed371df`,
PRD 29.1/29.2.1/29.2.2 and architecture A06/A07. The four controls and their
session/source rules are documented in [CONFIGURATION.md](CONFIGURATION.md#visual-session-preferences).
These corrections preserve the selected wordmark, palettes and responsive layout;
they add no customization controls.

Two focused regressions failed before correction: theme Reset changed hidden
preferences, and CLI lock feedback was not derived from the selected entry.
The final focused checks passed on the uncommitted C1 correction tree based on
`4894633edb64945e6d5647b74e76b1312db93aed`: 70 TUI tests, eight Python terminal-suite
tests and workspace formatting. Tests cover theme-only Reset with/without a CLI
lock, Cancel/Apply, retained composer/focus/scroll, unchanged Developer Menu Reset,
and selected-entry lock feedback at 60x16, 80x24, 120x40 and 160x50. An intermediate
regression also caught stale lock text outside the dialog; lock feedback now
belongs only to the selected entry. A formatting check required one adjustment.

Commands run from the application repository root:

```sh
cargo test -p fluzo-tui --locked --offline
cargo fmt --all -- --check
python3 -B -m unittest discover -s scripts -p 'test_tui.py'
```

Existing tests also cover 0/15/30/60 FPS, reduced motion, invalid input,
preview rollback, full/cached rendering equivalence and terminal-capability
fallbacks. These are synthetic component/PTY checks, not real-runtime acceptance,
a new manual contrast review or reference performance evidence. Live inference
is `not_run`. No issue closure or remote publication is implied.

A wide preview-status row previously painted over the middle of the wordmark.
It is now confined to the conversation width; regression buffers cover all
synthetic states at 120x40, 160x50 and 196x36, including the untouched relief rows.

Select `Notification test message` and Enter, or press Ctrl+E, to edit a separate
synthetic notification body. Arrow/Home/End/Delete/Backspace and sanitized paste
edit it; Ctrl+U clears it, Esc returns without scheduling, and Enter schedules
one test after three seconds. Ctrl+T schedules the currently entered text directly.
The text is capped at 256 UTF-8 bytes, control sequences are removed and OSC
separators are normalized. It is never copied from the composer or persisted.
Use synthetic text only, never secrets: desktop notifications leave the app.

Desktop delivery still requires `--desktop-notifications`, Ghostty, and an
observed focus loss before the deadline. Unknown/focused windows suppress it;
successful terminal writes do not prove a visible popup. The application does not
add a native desktop backend or an in-app notification stack. Keyboard/buffer
regressions cover navigation, editing, draft preservation and logo composition;
the PTY test sends custom pasted text and verifies the exact sanitized OSC bytes.

### C2 notification logic without visual integration

The non-visual portion of [C2 (#37)](https://github.com/fluzo-labs/fluzo/issues/37)
adds `fluzo_tui::notification_stack::NotificationStack`. This is a reusable owned
presentation model, not a renderer or an application-port change. The interactive
shell, keyboard bindings, artwork, colors, layout and existing desktop-test
transport are unchanged. The shell does not instantiate the new stack yet.

- Each stack consumes one ordered event stream in a fixed application cursor
  epoch. The host must supply strictly increasing sequence numbers for distinct
  notices, in delivery order; gaps are allowed. It must not mix independent
  streams, reuse a cursor for distinct outcomes or deliver unseen older events.
  Epoch changes fail explicitly and require a fresh stack after host resync.
- A fixed-size sequence watermark rejects duplicate/stale delivery even after
  expiry, dismissal or overflow, without an unbounded deduplication index. Equal
  text with distinct increasing cursors is retained independently. Coalescing
  preserves the original text, severity and deadline and cannot resend externally.
- At most 32 notices, including pending ones, are retained. Text is sanitized
  incrementally with the existing terminal sanitizer, made single-line, and capped
  at 1024 UTF-8 bytes including `...` when truncated, at a grapheme boundary.
  A very large single grapheme may reduce to the indicator alone. Text storage
  uses bounded boxed strings; metadata consists only of fixed-size IDs, enums
  and timestamps. No raw payload, callback or authoritative task state is retained.
- Overflow considers the incoming notice too: informational notices are discarded
  first, then success, warning, error and approval, oldest first within a priority.
  A lower-priority incoming notice cannot evict a higher-priority retained one.
  A saturating aggregate counter records overflow without generating more notices.
- Transient deadlines start at receipt, including while pending. The host supplies
  elapsed monotonic time to `receive`/`advance` and advances before reading a view.
  Reversed time and deadline overflow fail without mutation. `UntilDismissed`
  records persist until explicit dismissal or bounded overflow; they are still
  only presentation records, not durable failure/approval history.
- `visible(available_slots)` returns the oldest retained entries up to the smaller
  of available capacity and configured maximum. Capacity can fall to zero and
  recover without changing configuration or deadlines. The future renderer owns
  geometry and must exclude composer, dialogs and permission controls when
  calculating slots; this model alone does not prove absence of visual overlap.
- Settings changes validate atomically through core; invalid changes preserve all
  prior state. Duration changes affect new notices only. Dismissal, expiry and
  overflow cannot approve, resume, cancel, execute or send anything. The caller
  retains authoritative failures and approvals independently (UI-04 integration).

The core boundary regression first failed on the previously unlimited duration.
After correction, core and TOML/draft regressions passed, along with ten deterministic
stack tests covering boundary values, pending expiry, duplicate replay after
removal, identical text with distinct IDs, overflow priority/age, saturating
counters, 1000-event bursts, bounded sanitized text, Unicode clusters, zero display
capacity, atomic settings rejection and invalid time/epochs. No sleeps, real
notifications, filesystem fixtures or live inference are used by these tests.

Focused commands from the repository root:

```sh
cargo test -p fluzo-core --locked --offline notification_bounds
cargo test -p fluzo-runtime --lib --locked --offline notification_ranges
cargo test -p fluzo-tui --locked --offline notification_stack
```

On the uncommitted non-visual C2 tree based on
`9c3a40451944e980891967d09e48ca10c2387817`, the complete local checks passed:
140 workspace Rust tests (80 TUI), 47 Python tests including eight existing PTY
paths, formatting, workspace check/build, Clippy with warnings denied, the
resolved dependency/skill checker and the isolated offline LSP fixture. Commands:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
cargo build --workspace --locked --offline
python3 -B scripts/check_dev_setup.py
python3 -B -m unittest discover -s scripts -p 'test_*.py'
python3 -B scripts/check_lsp.py
```

These results are local, not remote CI or visual acceptance. No new dependencies,
protocol types, renderer code or application execution paths were added.

The 1-30 second and 1-5 visible settings limits now apply to shared configuration
validation; see [CONFIGURATION.md](CONFIGURATION.md#bounded-notification-settings).
This does not complete C2: visual preview placement, reversible notification
controls and new buffer/PTY acceptance scenarios require separately authorized
integration. Existing desktop test text remains capped at 256 bytes. Save/apply
persistence remains C3, and full visual/runtime acceptance remains later work.

### Follow-up validation evidence

The follow-up working tree based on `a7ff984` was validated before publication,
with the same application content as this delivery. All commands in the
stabilization verification section passed again: 126 Rust tests (68 TUI),
47 Python tests including eight PTY cases, format/check/build/Clippy, development
checks, isolated LSP and the private-loopback HTTP profile. Live inference was
`not_run`. The tests do not require private credentials or model services.

Additional local probes passed 32 PTY combinations at 120x40: 0/15/30/60 FPS,
reduced motion on/off, reversible default/high-contrast preview and session
application, high-contrast RGB, ANSI and NO_COLOR. They checked idle output
silence, typing during synthetic playback, cancellation, retained drafts and
paused history, unchanged temporary source/configuration, and terminal cleanup.

Eighteen emulator-hosted relay scenarios passed in Ghostty 1.3.1-arch2.2 and
Alacritty 0.17.0 (94e7c887), using X11 with temporary HOME/configuration: each
terminal at 80x24, 120x40 and 160x50 with default RGB, high-contrast RGB and
NO_COLOR. The probes captured 54 window-only images and 18 synthetic output
recordings. Input was injected into an inner PTY, not through real emulator
keyboard events. Ghostty initial dimensions required calibration of the owned
window; some images include its temporary resize badge. These are limited
presentation checks, not complete human acceptance or physical display metrics.

A separate debug-build probe filled the 256-line retained-history limit using
ten 32-line batches of approximately 223 bytes per line, then exercised normal
700-ms synthetic playback, input, resize and cancellation. The host used Rust
1.98.0, Python 3.14.7 and CachyOS kernel 7.2.6-1-cachyos. Each mode measured 50
single-character samples from PTY injection to the exact composer cell appearing
in captured output, including Python observer overhead. p95 uses nearest rank
(sorted index 47); this is not a release-reference profile.

| FPS | Reduced motion | Median ms | p95 ms | Maximum ms | Cancel ms |
| --- | --- | --- | --- | --- | --- |
| 0 | off | 29.524 | 35.941 | 36.369 | 26.304 |
| 15 | off | 30.409 | 40.660 | 61.856 | 27.263 |
| 30 | off | 33.476 | 52.648 | 63.345 | 29.555 |
| 60 | off | 33.255 | 40.338 | 65.864 | 27.508 |
| 60 | on | 29.510 | 35.086 | 35.380 | 28.967 |

Eight-character bursts separately reached p95 292-302 ms. All final probes
completed with restoration, but input/render serialization needs profiling.
One exploratory single-key p95 exceeded 50 ms. PRD 49 acceptance remains
unverified: the required release build, approved hardware/profile, 1000 samples,
60-second high-rate event workload, CPU/render distributions and persisted
large-history fixture were not supplied by this demo probe. Lower-rate results
must not substitute for the default-60 reference gate.

Review also flagged subdued secondary model/footer text in the default-theme
Alacritty capture. Full semantic-state/diff contrast review remains pending.
The maintainer confirmed Ghostty desktop delivery after testing; Alacritty has
no notification adapter and is not counted as notification support.

Failed harness attempts were not counted as passes: stale screen markers,
incorrect assumed editor row, initial Ghostty cell sizes, blocking PTY writes,
a 4 MiB capture cap and a broad single-character marker. The final probe used
nonblocking writes with bounded draining, rolling capture, exact-cell matching
and an external 45-second case deadline. No application code was changed during
those validation probes. The local synthetic artifacts and harnesses remain in
`/tmp/fluzo-ui-validation.M9qe4F/`; they are not committed or publicly hosted,
so this summary is not a claim of durable screenshot/recording evidence.

### Stabilized behavior

- The original FLUZO wordmark and its 30x3 relief geometry remain intact. Pending
  and Ready are dim, Running pulses inward, Waiting pulses amber slowly, Blocked
  and Stopping are static amber, Completed highlights green for one second then
  settles, Failed is red, and Cancelled is gray. Compact and large presentations
  use the same state policy. Text labels remain independent of color. Synthetic
  playback has a Running presentation override, not a changed task snapshot.
- Animation uses monotonic elapsed time and the existing 60/30/15/0 scheduling
  controls. Reduced motion, FPS 0 and modal wordmarks are static; Waiting has an
  animation deadline and Completed loses it after one second. Failure/cancel
  colors are static, not brief animated fades. The separate top activity line
  continues during work above modals. No real execution is implied.
- The existing `tui.flags.render_diagnostics` preference now visibly controls a
  dedicated last-screen-row strip, below the contextual footer and outside the
  editor/dialogs. It reports session UI version, configured animation target,
  lifetime-average application redraws/second, previous draw duration in
  microseconds and skipped animation deadlines. Values refresh on existing
  redraws only; enabling diagnostics does not start an idle refresh loop. These
  are not physical display FPS, latency percentiles or a performance benchmark.
- Full renders format retained conversation once and return bounded geometry
  containing the viewport and optional visible loading row. The terminal owns
  that geometry with its cell-buffer cache. Animation-only draws repaint that
  row directly and never call the retained-conversation formatter. Input, resize,
  scroll, expansion, theme/snapshot changes, timer seconds and scrollbar expiry
  invalidate the full frame. Editor cursor layout is still recomputed; this is
  not a claim of zero allocation or measured CPU improvement.
- A final buffer color pass covers all widgets, Markdown, metric bars, dialogs
  and relief backgrounds. Without detected truecolor it maps RGB to named ANSI
  colors; nonempty NO_COLOR resets foreground/background colors. Terminal palette
  customization still affects actual contrast and requires visual review.
- Paused history shows an explicit new-source-line count in the footer. Incoming
  activity preserves anchor, draft and focus; Follow clears the count. NORMAL
  remains a read-only demo label in the compact status or wide sidebar, not an
  approval control. Ignored mouse events no longer skip host timers or force a
  full redraw.

### Scope boundaries and acceptance still required

D0 reconciled the selected FLUZO artwork with the design baseline at
`fluzo-docs` revision `53b345e9f1782beef839b42b4c0de4773ed371df`. That documentary
decision preserves the existing appearance; it does not establish parent UI-02
acceptance or authorize a redesign.

Extra logo speed/intensity controls, manual compact/wide selectors, additional
accent/contrast presets and layout/tool presentation selectors are optional and
not selected. They are not C1 acceptance gates. Only the four existing typed
visual preferences are editable (the separate notification test text is not a
setting); preview/apply/reset/cancel remain session-only and cannot grant runtime
authority. Error and cancellation use distinct static tints with independent
state labels; no new fade animation is required or introduced by these fixes.

C2 owns bounded notification preferences and isolated previews. C3 owns integration
with shared save/apply and configuration provenance, including restart presentation
where applicable. Real Laya consent/capacity integration remains later work; the
prototype does not expose a fake operational control. C4 owns the combined UI-02
acceptance evidence. These boundaries do not claim those phases are implemented.

Setup, config discovery, atomic save/reload/conflict handling and persistent
preference integration belong to the UI-03 boundary; they are not implemented
or claimed by this stabilization. Tasks, permissions, cancel-versus-force-stop
and complete notification UI belong to UI-04. Accumulated chat/diff/playback and
opt-in Ghostty OSC notification examples are synthetic partial demonstrations,
not completion of those stories. No provider, tool or configuration I/O is added.

UI-05 still requires recordings and human review on Ghostty and Alacritty,
including contrast and the required viewports. Reference performance, final
visual acceptance and remote evidence publication are separate pending gates.
Previously observed notification delivery to the desktop service does not prove
a popup was shown or explain the original missing-popup report. No issue, PR,
branch, index or remote state is changed by this increment.

### Stabilization verification

The local working tree based on `9d7f7b6465b44dc8987c9f9895baddf02e787546`
contains the accumulated visual work plus this bounded stabilization. Focused
Rust tests cover state colors and relief shadows, static/no-color fallbacks,
Waiting/Completed deadlines at 0/15/30/60, reversible visible diagnostics at
60x16/80x24/120x40/160x50, all-widget ANSI fallback, unread/follow behavior and
cached/full buffer equality. A formatter invocation counter verifies one history
format pass per full render and none per animation-only render, including reflow,
scroll and tool expansion. The cache remains host-owned; direct state mutation
requires a full render before reusing geometry.

Initial verification caught missing Ready/Stopping match arms, old tests assuming
idle and frozen-running colors were identical, palette tests missing explicit
truecolor capability, and a test caller using the wrong scroll signature. Those
failures were corrected and retained as part of this evidence history, not
reported as successful runs. A later ANSI regression caught a near-neutral rule
mapping to blue; neutral classification now keeps grayscale surfaces and avoids
collapsing distinct foreground/background colors on occupied cells.

Final local verification passed on the stabilization working tree: 121 workspace
Rust tests (63 TUI), 47 Python tests including eight PTY paths, workspace
check/build/Clippy with warnings denied, formatting, development graph/skill
checks, the isolated cross-crate LSP fixture and the loopback-only HTTP profile.
The PTY visual paths now enable the real diagnostics preference and observe its
last-row output at both FPS 0 and FPS 60. Commands run from the repository root:

```sh
cargo test --workspace --locked --offline
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo build --workspace --locked --offline
cargo fmt --all -- --check
python3 -m unittest discover -s scripts -p 'test_tui.py'
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_dev_setup.py
python3 scripts/check_lsp.py
python3 scripts/test_http_simulator.py
git diff --check
```

No manifests, dependencies or lockfile changed in this stabilization. Its changes
are confined to `identity.rs`, `visual.rs`, `shell.rs`, `terminal.rs`, the existing
PTY tests and this document; other accumulated dirty files remain untouched.
The index is empty and HEAD is unchanged. Live inference is `not_run`; remote CI,
formal Ghostty/Alacritty acceptance and reference performance are unverified.

## Current visual direction: Crush-style conversation

The latest maintainer request supersedes the earlier custom header/layout studies
below: reproduce Crush's conversation aesthetics and layout before adding Fluzo
visual differences. The running demo now uses a borderless conversation and editor,
without the earlier boxed Composer or permanent sidebar divider.
At the maintainer's request, the original three-row FLUZO relief wordmark is
restored in the sidebar, with its cyan/blue/violet gradient and darker shadow.
It occupies 30 columns without changing the responsive layout. Its slow gradient
moves during work, with the state tints and pulses defined above; it stays static
when idle or a modal is open and honors FPS 0 and reduced motion. Compact mode retains the single-line FLUZO header with
per-letter colors sampled from the large wordmark's gradient at the same animation
phase. It shares the work/idle, modal, FPS 0, reduced-motion and color-capability
rules. ASCII mode keeps ordinary FLUZO letters; NO_COLOR removes their colors. The original top-edge loader is also restored: it spans
80% of the viewport, travels left/right/left in 0.8 seconds during work and uses
the faster 0.3-second gradient phase. Idle is centered and static; FPS 0 and
reduced motion disable movement. It uses the existing first row without changing
message/editor geometry and remains visible above modals. Unit and PTY tests
pass, including cached/full-render agreement and resize. Historical evidence
below describes earlier iterations.

Source analysis used `charmbracelet/crush` revision
`1f3827bcd2d20f38076b2d46123683271e6ed9ba`:

| Source | Observed presentation contract | Local implementation |
| --- | --- | --- |
| `internal/ui/model/ui.go` | One-cell outer margins, 32-column sidebar allocation; compact below 120 columns or 30 rows | Responsive chat layout with the same breakpoints and sidebar allocation |
| `internal/ui/chat/messages.go` | Two-column message gutter, readable content capped at 120 columns | Wrapped display rows retain source identity and continuation position |
| `internal/ui/styles/quickstyle.go` | Purple user rule, thick focused rule, unbordered assistant, thin status icons, muted tool panels | User/assistant/tool presentation, pending/success/stopped states, full-block selection |
| `internal/ui/chat/tools.go` | Ten-line collapsed tool body with expansion | Bounded retained output; Enter/Space or a second click toggles, Space toggles expansion |
| `internal/ui/model/ui.go` editor prompts | Four-column prompt, `  > ` first focused row, `::: ` continuation; 3..15 rows | Soft-wrapped growing editor, additionally capped to preserve a usable small viewport |
| `internal/ui/dialog/common.go`, `commands.go`, `quit.go`, `common/elements.go` | Internal gradient title, spaced search/list/footer, compact quit with padded buttons | Shared rounded frame, internal diagonal title strip, search cursor, selected rows and separate help; compact safe-discard confirmation |
| `internal/ui/styles/themes.go` | Charmtone Pantera palette | Pepper #201F26, Sash #ECEBF0, Squid #858392, Charple #6B50FF, Julep #00FFB2, Malibu #00A4FF and BBQ #2D2C36 |

Palette values were checked against the referenced
`github.com/charmbracelet/x/exp/charmtone` v0.1.0 source. This is an independent
Ratatui implementation, not a Go source translation or a copied Crush distribution.
The inspected Crush revision's `LICENSE.md` is **FSL-1.1-MIT**, not an immediately
unrestricted MIT grant: it restricts competing commercial use and provides a
future MIT grant after two years. No Crush source, logo artwork, branding or
license-restricted assets were incorporated. Licensing review is necessary before
any future source reuse or distribution that relies on that grant; no legal
clearance or exact-clone compatibility is claimed here.

The synthetic transcript distinguishes user messages, assistant paragraphs and
tool results. The completion footer now follows the inspected
`AssistantInfoItem` and `common.Section` presentation: a two-cell gutter, diamond,
model name, `via` provider, `in` duration, then a muted horizontal rule filling the
remaining viewport width. It is a separate non-highlighted row, not tool-body
content; it has no extra token-count row. Model/provider/duration remain static
example values in this offline demo. Running and stopped blocks have no completion
footer. The demo treats each completed synthetic block as a completed example
turn, not evidence of actual inference. Earlier footer descriptions below are
historical. Basic headings, emphasis, inline code, lists, quotes and fenced
code/diffs render with Unicode-aware wrapping. This is not a CommonMark/Glamour
replacement: nested Markdown, tables, syntax-language highlighting, attachments,
text-range dragging/copying, all specialized tool cards and the complete Crush
landing/settings flows are not implemented. The FLUZO wordmark remains an
independent relief artwork rather than Crush artwork. No provider or tool is
connected merely to demonstrate these surfaces.

An inline loading wave follows the active synthetic message, in the same
presentation position as Crush's assistant spinner. Eight block bars separated
by seven spaces form a 15-cell sound wave with a 1.2-second cycle and the Fluzo gradient; ASCII mode
uses `.`, `_`, `=` and `#`. The adjacent `Working (demo)` label smoothly cycles
through the same palette over 2.4 seconds, without changing text or width. Its
color animation shares the wave's motion controls and honors NO_COLOR.
It is transient presentation, never retained history
or completion evidence. Completion/cancel removes it. FPS 0, reduced motion and
open modals freeze it; cached redraws update only its visible row without moving
focus or resetting scroll. The separate top loader and sidebar logo remain.
Unit tests cover phase/width, fallbacks, lifecycle and cached/full-frame equality.
PTY tests observe the inline loading label; one run hit the previously observed
resize/search timing failure before playback, followed by a passing six-test run.

Use Ctrl+P, then Play synthetic stream to inspect the transcript. The second
synthetic block includes formatted assistant text, a diff and long expandable
output. Click selects a whole message; Tab switches to conversation keyboard
focus, Shift+Up/Down or K/J visits messages/tools, Space toggles tool details. Wheel and
PgUp/PgDn scroll rendered rows; End resumes following. Resize keeps draft and
selection, and clamps continuation anchors within their retained source line.
Enter sends a synthetic preview, never a runtime command, and retains the input.
Ctrl+J or Shift+Enter inserts a newline. The earlier bindings below are historical.

### Current dialog composition

The dialog pass uses the same pinned Crush revision above, including the
`quickstyle.go` dialog styles. Titles now sit inside the frame rather than on its
border, followed by a Charple-to-Dolly diagonal gradient. Search inputs have their
own row and vertical margins; action rows use the full list width with a purple
selection background and Butter foreground. A separate muted footer keeps the
available controls visible. Commands, theme/developer previews and read-only
model/session/details views share this composition. No fake provider lists or
System/Custom/MCP categories are added.

Standard dialogs remain at most 70x20 with two-cell screen margins. At 60x16,
search margins collapse to keep all seven available actions and the footer
visible. The filter scrolls horizontally at grapheme boundaries, reserving a
cursor cell; animation-only draws retain the same cursor and modal contents.
The quit confirmation is a compact 50x10 frame with centered question, spaced
padded buttons and two hint rows. Its selected button uses Dolly, its inactive
button Char. Keep editing remains the initial choice; repeated keys and paste
cannot confirm discard. Unlike Crush, there is no double-Ctrl+C discard bypass
or modal mouse activation. Ctrl+G retains the specifically requested borderless
help table. The custom FLUZO logo and loaders are unchanged.

Verification for this dialog pass on the uncommitted `9d7f7b6`-based tree:
103 workspace Rust tests (45 TUI), 45 Python tests (including six PTY paths),
workspace check/build/Clippy, formatting, development checks, isolated LSP and
`git diff --check` passed. New composition regressions first failed against the
previous title placement and quit layout, then passed after implementation.
Checks cover 60x16, 80x24, 120x40 and 240x80, all overlay cache/full-frame
agreement, Unicode search, ASCII/NO_COLOR/high contrast, safe quit and read-only
information views. PTY checks now wait for the relocated search prompt and
exercise Keep editing before deliberate discard. Earlier timing failures remain
recorded; these passes do not prove all schedules or visual identity. Exact
screenshot parity, mouse parity and UI-02 acceptance remain unverified.

### Model information in sidebar and compact mode

The model pass follows `model/sidebar.go`, `common/elements.go`, `model/header.go`
and `drawSessionDetails` at the same pinned Crush revision. The sidebar shows a
muted diamond, bright model name, provider on the same row only when it fits,
then indented `Reasoning X-High`, context percentage, token count and cost.
The provider otherwise occupies its own indented row. Token count and reasoning
use the darker Oyster shade; provider, percentage and cost use Squid. Values
remain the existing illustrative offline fixture, not measured inference usage.

Compact mode follows Crush's different information hierarchy: the header shows
working directory, context percentage and `ctrl+d open/close`, not the model name.
The original compact FLUZO gradient and top loader remain. Diagonals fill available
space; paths retain at most four components and the details text truncates safely
at grapheme boundaries. Ctrl+D opens a full-width panel immediately below the
header, reusing the same model presentation with the provider inline when it fits.
Ctrl+D or Esc closes it without changing the draft or executing anything. Wide
mode retains its existing details dialog; only compact details use the top panel.
The demo's state remains on the next header row, and theme/developer previews
retain visible synthetic-state feedback even when they cover that row.

This pass passed 104 workspace Rust tests (46 TUI), six focused PTY checks,
Clippy and build. Regressions exercise shared model text/styles, responsive
provider placement, compact panel toggle, Unicode truncation, sidebar hiding,
all-overlay cached redraws and unchanged draft/runtime authority. PTY testing
initially exposed obscured developer-state feedback; restoring its visible
preview-header position fixed the regression. Exact visual parity and dynamic
provider/context integration remain outside this synthetic presentation pass.

### Model metric bars

Context usage and cost now occupy separate rows in the sidebar and expanded
compact details. Each has an aligned 12-cell horizontal bar followed by its value:
`75% (303.8K)` and `$50.00`. Context fills 75% of its bar; cost uses an explicitly
labeled `$100 (demo)` visual scale, not a configured or enforced budget. The compact
single-line header retains its percentage summary. Unicode uses heavy filled and
thin remaining strokes, with `#`/`-` in ASCII mode; value text and stroke shape
remain readable without color. No budget setting or runtime authority is added.
47 focused TUI tests, six PTY checks and workspace Clippy passed for this pass,
including separate rows, equal bar widths, value alignment and small layouts.
The X11 Ghostty preview remains the verified workaround for desktop resize bounce;
no persistent terminal configuration was changed.

### Updated help dialog and application footer

The latest help request supersedes the earlier borderless outer panel: Ctrl+G
now shares the other dialogs' rounded purple frame, internal gradient diagonal
title, padding and separate navigation footer. The three-column table itself
still has no grid rules. Pagination reserves the frame and keeps every shortcut
reachable at 60x16. The common title renderer is shared rather than approximated.

The application footer now follows Crush's `Status.Draw`, `UI.ShortHelp` and
help palette at the pinned revision: muted keys, darker descriptions, dot
separators, one-cell margins and contextual hints. Tab describes the destination
focus; an empty editor advertises `/ or ctrl+p`; active playback puts Esc cancel
first; modals show Esc close instead. Unsupported modes are not advertised.
The former permanent NORMAL/WORK/IDLE banner is removed. A small right-aligned
`demo following/paused` indicator remains a deliberate demo difference, including
truncation feedback. Hints that do not fit are omitted as complete entries;
Ctrl+G still provides all supported bindings. Notifications remain in the
existing editor status row, not Crush's timed notification overlay.

106 workspace Rust tests (48 TUI), 45 Python tests including six PTY paths,
Clippy and build passed. Tests cover table alignment/pagination, shared frame
and title, contextual hints, width limits, draft retention and cached redraws.
Exact screenshot parity and runtime acceptance remain unverified.

### Tab and pointer focus

The latest focus pass follows Crush's editor/chat Tab dispatch and focused/blurred
textarea and message styles. Tab from the editor focuses the chat, selects its
latest retained message/tool and reveals the bottom. Tab back preserves the
selection identity but removes its visual focus rule, restores editor colors and
cursor, and retains the draft. The blurred editor uses gray text and prompts;
the footer changes its focus destination and navigation hints. Plain Tab alone
changes focus; overlay-local Tab behavior is unchanged.

Clicking a message now focuses chat as well as selecting it; clicking the editor
returns focus without moving its existing cursor. Wheel scrolling alone still
does not steal focus. These rules supersede earlier click-without-focus notes.
There is still no separate sidebar focus or exact Crush text-selection parity.
49 focused TUI tests, six PTY checks, Clippy and build passed. The PTY fixture now
returns from clicked chat to editor with Tab before pasting, rather than assuming
that clicking a message leaves editor focus unchanged.

### Completion notifications and testing

Research at the pinned Crush revision covered `ui/notification/{notification,
native,native_beeep,osc,bell}.go`, backend selection, focus policy and
`handleAgentNotification`. Crush suppresses notifications while focused and
supports native OS delivery, OSC 99/777, bell and disabled backends. This first
Fluzo adapter implements its OSC 777 path for Ghostty only, not native D-Bus,
OSC 99 capability negotiation or bell fallback. No dependencies or subprocesses
are added. Other terminal types fail closed with an explicit unavailable result.

`--desktop-notifications` explicitly enables the existing core desktop setting
for this session. The default remains disabled. The terminal guard enables and
restores focus reporting; unknown focus is treated as focused. Only an observed
focus loss allows delivery. A notification is emitted once at natural synthetic
playback completion, not on individual tool fragments, cancellation, snapshot
preview, redraw, focus change or exit. Finalization completes any remaining
synthetic fragment before emitting the completion message. Fixed text labels all
messages as synthetic, never leaking paths, drafts, session titles or secrets.
All output uses the terminal owner, outside synchronized frame updates. Delivery
errors are surfaced without retry; accepted bytes do not confirm desktop display.

Manual test: start `demo --interactive --dev-menu --desktop-notifications`, open
Developer menu with Ctrl+P, press Ctrl+T and focus another window within three
seconds. One pending test is allowed; another request replaces its deadline.
Keeping focus suppresses the test. The test remains scheduled after dismissing
the menu but is discarded on application exit. The host desktop controls display,
sound, duration and stacking; core duration/max-visible values are not advertised
as enforced by OSC 777. Actual runtime completion wiring awaits the native runtime.

Verification: 112 workspace Rust tests (54 TUI), 47 Python tests (eight PTY),
Clippy, build, formatting and development checks passed. Unit tests use an
in-memory/failing writer and injected elapsed time. PTY tests use an isolated
HOME without desktop bus access and capture exact OSC bytes, focus suppression,
disabled mode, cancellation, natural completion and focus-mode restoration.
The natural completion case takes approximately 56 seconds. Initial test failures
were harness errors (raw diff matching and combined Esc/Ctrl+C input), corrected
with screen reconstruction and a cleanup acknowledgement. Desktop popup delivery
and visual acceptance remain manual checks, not inferred from captured bytes.

### Faster loading wave and elapsed time

The inline wave now completes its cycle in 0.6 seconds, twice its previous speed.
Its label is followed by elapsed playback time (`0s`, `12s`, `1m01s`), measured
from the explicit start using monotonic elapsed time. The timer continues across
synthetic fragments, resets on a new playback after stopping and disappears with
the loading row. One-second presentation updates remain active at FPS 0 and with
reduced motion; the wave itself stays frozen under those settings. Logo/top-loader
speeds and the label color cycle are unchanged. 55 focused TUI tests, eight PTY
checks, Clippy and build passed, covering cycle duration, timer reset, minute
formatting, disabled animation and terminal lifecycle.

### Desktop delivery investigation

Following a reported missing popup, a real Ghostty/X11 playback was exercised
through an isolated PTY relay with synthetic input. Its focus-loss sequence and
OSC emission were observed, and a filtered session-bus monitor counted exactly
one `org.freedesktop.Notifications.Notify` call containing `Fluzo is waiting...`.
A direct OSC probe also reached the desktop service. Plasma reported notifications
not inhibited. This establishes delivery to the desktop service for the probe,
not that the reported missing popup was reproduced or visually displayed.

The previously silent completion path now reports whether the notification was
sent to the terminal, disabled, unsupported, suppressed due to unknown focus,
suppressed because the terminal still reports focus, or failed while writing.
It does not claim desktop display from successful writes. Five focused policy
and option tests, eight PTY tests, Clippy and build passed for this diagnostic
change. No desktop/terminal preferences were changed; the original missed popup's
cause remains unconfirmed.

### Chat scrollbar alignment

The scrollbar now follows Crush's `common/scrollbar.go`, glyph constants and
`Chat` default visibility policy at the pinned revision: a Dolly `┃` thumb over
a Char `│` track, proportional size/position, visible after navigation and hidden
after two seconds without renewed navigation. Incoming output does not extend
that deadline. ASCII uses `#` and `|`; monochrome/high contrast retain distinct
shapes. The host invalidates the frame on visibility changes even at FPS 0.
Fluzo keeps its reserved column to avoid reflow when the scrollbar disappears;
Crush's optional always/never modes and resize cache policy are not reproduced.
56 focused TUI tests, eight PTY checks, Clippy and build passed, including exact
thumb/track colors, geometry, timeout, renewed navigation and non-overflow reset.

### Current keyboard bindings

Ctrl+G presents a rounded outer dialog with a borderless, three-column help table: Context, Shortcut and
Action. Each action occupies its own row, with aligned columns and distinct key
styling. PgUp/PgDn (also arrows or Tab) changes pages; Ctrl+G/Esc closes it.
Pagination adapts to terminal height so all shortcuts remain reachable at 60x16
without clipping descriptions or changing the composer draft.

Compared against `internal/ui/model/keys.go` and dispatch priority in `ui.go` at
the pinned Crush revision above. These mappings replace conflicting demo keys;
they do not implement missing runtime capabilities.

| Scope | Keys | Demo behavior |
| --- | --- | --- |
| Global | Ctrl+C | Quit; nonempty draft still requires deliberate discard |
| Global | Esc / Alt+Esc | Close overlay first; otherwise stop playback, or clear selection |
| Global | Ctrl+P | Commands; `/` also opens them in an empty composer |
| Global | Ctrl+G | Toggle help |
| Global | Tab | Composer/conversation focus |
| Global | Ctrl+B | Toggle sidebar when the terminal meets 120x30 |
| Global | Ctrl+D | Read-only session details |
| Global | Ctrl+L / distinct Ctrl+M | Read-only model information |
| Global | Ctrl+S | Read-only session information |
| Composer | Enter | Send synthetic preview, retaining draft; repeats do not resend |
| Composer | Ctrl+J / Shift+Enter | Newline, never submit |
| Composer | Ctrl+A/E/H/U/K | Line start/end, backspace, delete before/after cursor |
| Composer | Up/Down | Bounded history for single-line drafts; multiline editing otherwise |
| Conversation | Up/Down, k/j, Ctrl+K/J | Scroll one rendered row |
| Conversation | Shift+Up/Down, K/J | Previous/next message |
| Conversation | PgUp/PgDn, b/f, u/d | Full/half viewport scrolling |
| Conversation | Home/End, g/G; global Ctrl+End | Top/bottom; bottom resumes following |
| Conversation | Space | Expand/collapse selected tool |

Ctrl+N (new session), Ctrl+Y/Shift+Tab (modes), Ctrl+T/Ctrl+Space (tasks), Ctrl+Z
(suspend), Ctrl+O (external editor), Ctrl+F/R/V (attachments/clipboard), selection
clipboard chords, chat c/y/C/Y and sidebar/horizontal navigation report unavailable.
They do not simulate consent, execute tools, spawn processes, or mutate persistent
sessions. `@` remains literal text: file completion is not implemented. Thus this
is key-map alignment for existing demo functions, not complete Crush parity.
Devmenu Ctrl+A/R/N remain explicitly local preview controls. Ctrl+Q/F1 and
Ctrl+S-preview are no longer the quit/help/send bindings.

Ghostty/Kitty enable disambiguated keyboard reporting with paired restoration,
allowing Shift+Enter and distinct modified keys. Other terminals retain Ctrl+J;
legacy Ctrl+M is indistinguishable from Enter and therefore sends a preview.
No terminal configuration is modified. PTY checks exercise help, model/session
views, Ctrl+J, encoded Shift+Enter, Esc cancellation, Enter preview, Ctrl+C exit
and keyboard-mode restoration. A partial-frame draft assertion was replaced with
bounded row readiness; the expanded four-line editor requires restoring 80x24
before observing full playback content. Existing resize timing failures remain
recorded above.

Verification on the uncommitted tree based on `9d7f7b6`: 92 Rust tests and the
45-test Python suite pass, including six PTY paths. Regressions cover responsive
breakpoints, 240x80 growth, no-signal resize, Unicode wrapping/cursor placement,
selection and scroll preservation, tool expansion, status glyph weights, modal
feedback and restoration. Tests exposed and corrected narrow-sidebar arithmetic,
cached overlay painting, obscured locked-setting feedback, and obsolete fixed-row
mouse coordinates after editor growth. Formatting and Clippy pass. Screenshot
parity and reference-terminal visual acceptance remain unverified; passing these
checks does not establish an exact clone, UI-02 acceptance or permission to publish.

## Earlier run and interaction contract

```sh
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive
```

This is an explicitly labeled synthetic workspace, not an agent session. The CLI
supplies the existing scenario driver's application port; TUI queries its owned
snapshot and rejects non-demo or incompatible snapshots. It never submits a
runtime command, reads `.fluzo`, loads credentials, contacts providers, runs tools,
or writes workspace files. Existing `demo`, help and version output are unchanged
apart from the new documented interactive option. Missing terminal stdin/stdout
fails promptly without emitting terminal controls; use `fluzo demo` for plain text.

| Input | Behavior |
| --- | --- |
| Printable text, bracketed paste | Insert into the composer; never dispatch or submit |
| Enter | Insert a newline |
| Arrows, Home/End, Backspace/Delete | Edit at grapheme boundaries; Up/Down retain grapheme column where possible |
| Ctrl+S | Append a clearly labeled user preview; retain the draft |
| Tab | Switch composer/conversation focus, without changing input destination |
| Ctrl+P | Search actions; Up/Down selects, Enter activates, Esc dismisses |
| F1 | Open help; Esc restores the same focus |
| PgUp/PgDn | Browse retained output without following new content |
| Mouse wheel over conversation | Scroll three retained lines; reaching the bottom resumes following |
| Left/Right in conversation | Pan long lines horizontally |
| End in conversation, Follow output action | Resume following; clear unread count |
| Ctrl+C | Stop synthetic playback; retain partial output and draft, not exit |
| Ctrl+Q | Exit; a nonempty draft requires deliberate Discard selection |

The exit prompt starts on Keep editing. Paste is ignored in overlays, so pasted
newlines and command names cannot activate an action. Repeated control/overlay
key events are ignored. Opening/dismissing overlays, resize and synthetic stream
updates preserve draft, focus and absolute retained-line scroll anchor. Eviction
clamps an old anchor to the oldest retained line and exposes truncation.

Palette actions are Play synthetic stream, Stop preview, Follow output, Help and
Quit demo, plus Theme preview and the conditionally enabled Developer menu.
Playback supplies presentation text only, stops after 80 presentation steps
(40 synthetic fragments) and never claims a real task completed. Each fragment
has an action title, a thin text checkmark (U+2713, explicitly non-bold,
no emoji selector; ASCII `[v]` fallback), and an indented body. The checkmark is decorative at startup, not
completion evidence: titles explicitly say Demo running/complete/stopped.
A second host tick finishes the fragment and appends `GTP-6 Astra | 1m21s` and
`2,480 tokens | example metrics`; these are illustrative, not measured usage.
Ticks are spaced 700 ms apart solely for visual inspection. Cancel retains the
partial body and marks it stopped without adding completion metrics.

Clicking a fragment selects its stable retained-line identity without changing
composer focus or dispatching work. A short horizontal accent stroke at the left
of its title marks selection. With conversation focus, Up/Down selects fragments;
wheel scrolling remains independent. Body text has two extra spaces after the
selection gutter. Titles, bodies and footer share selection hit testing; history
and fragment metadata remain bounded and selection clears when its item expires.
Unit/PTY checks cover clicking, keyboard selection, completion-only example
metrics, cancellation, history eviction and preserved drafts. It is not a second execution engine or a replacement
for future provider/application updates. A Cancel key here means stop preview,
not proof of remote cancellation. Session/task selection, authorization dialogs,
force stop, tools, operational settings editing and durable history are not implemented.

## Rendering and terminal ownership

The shell separates editing/navigation state from the owned application snapshot.
Ratatui 0.29.0 supplies viewport rendering and differential output; Crossterm
0.28.1 owns input and terminal controls. Exact versions, Unicode editing/width
helpers and signal-hook are pinned. The selected Ratatui backend enables
Crossterm's default features transitively, including conditional Windows packages;
this does not establish Windows support. The graph checker checks package/version
pairs where two versions coexist and prevents terminal libraries reaching core or
runtime through normal/build edges. No new crates or workspace features are added.

The four regions are a compact synthetic-mode header, conversation, fixed-height
composer and status/shortcuts. Test viewports are 80x24, 120x40 and 160x50; 60x16 is
the minimum layout. Smaller sizes show a resize/exit notice and retain input in
memory. Long draft lines pan to the cursor; long conversation lines have keyboard
panning. UI-02 part 1 adds shared ANSI theme tokens, high contrast and text-based
focus/status. Nonempty `NO_COLOR` disables colors. No special font, emoji,
truecolor, clipboard or hyperlink support is required.

One coordinated writer redraws dirty state or due active motion; there is no idle
animation loop.
Input is checked before at most one due synthetic fragment. Signal observation
uses a bounded 50 ms event wait, not measured input/display latency. Synchronous
terminal writes can still delay rendering on a slow terminal; this slice makes no
runtime control-path or PRD performance claim. Drafts are capped at 8192 bytes,
retained output at 256 lines of at most 512 bytes each, palette queries at about
64 bytes and incoming preview chunks at 2048 characters.

The incremental sanitizer retains only a finite control-parser state, discarding
CSI/OSC/DCS and C0/C1 controls, expanding tabs and visibly escaping bidirectional
format controls. It strips rather than interprets provider styling. It preserves
normal multilingual text. Fragmented/incomplete controls cannot escape into raw
terminal output. It accepts valid Rust strings; provider byte decoding and secret
redaction are separate future adapter responsibilities. Crossterm assembles paste
before delivery; the application rejects oversized paste after that assembly, so
this is not a claim of bounded memory against an arbitrarily hostile terminal.

The host restores bracketed paste, cursor, alternate screen and raw mode
independently on normal exit or returned errors. It catches unwinding panics around
the session and restores before returning a generic diagnostic; previous panic
hooks and signal registrations are restored/unregistered. SIGTERM/SIGHUP/SIGINT
request cleanup. Ctrl+Z suspension is not implemented and does not enable a shell
handoff. SIGKILL, abort and hardware failure cannot guarantee restoration; a user
may need their terminal's reset action. No interactive child process is launched.

## Body and sidebar layout

The body on the left contains conversation and composer; the right sidebar shows
FLUZO, the synthetic session name, repository directory and model status in that
order. The sidebar is 34 columns at widths of 80 or more, and 26 columns at the
60-column minimum (with a plain wordmark instead of clipped artwork). The top
activity line and bottom status remain viewport-wide. Cursor placement and cached
animation use the same body geometry; overlays preserve input and sidebar state.
The sidebar divider includes an accent-colored scroll thumb alongside the
conversation viewport. Its size reflects the visible fraction of retained lines;
its position tracks scrolling and live following. It is hidden when all content
fits. Monochrome uses the thicker line shape; ASCII mode uses `#`.

The CLI supplies its current directory as an owned display string, abbreviating
the HOME directory to `~` using path-component matching (not string prefixes),
without Git subprocesses or config loading. It is not a verified Git root when launched from
a subdirectory. The TUI bounds/sanitizes it and shows at most two wrapped lines
with explicit truncation. Session and directory values have no separate labels.
The model block uses the user-requested illustrative text `GTP-6 Astra (ExtraHigh)`,
`via Github Copilot` and `75% (303.8K) $50.00`, followed by `Example data / offline`.
These are static visual fixtures, not measured usage, billing or a connection.
Actual session/model metadata still depends on the future runtime contract.

Layout verification: 81 Rust tests and 45 Python tests pass, along with Clippy,
formatting, graph checks, build and isolated LSP checks. Tests cover sidebar order,
bounded hostile directory text, body cursor confinement and cached/full-frame
agreement at all supported sizes. An initial cached-modal artwork mismatch was
fixed by updating its visible rows. A PTY run exposed repeated-key timing during
resize; the fixture now acknowledges an intervening search edit before requesting
the next synthetic state. Focused and full Python suites then passed.

### Resize and Crush comparison

The terminal host observes the actual terminal dimensions on every event-loop
iteration, independently of resize notifications. A changed size invalidates the
cached frame and resizes the Ratatui viewport. The demo imposes no maximum window
size; terminal/backend coordinate and memory limits still apply. Below 60x16, a
compact minimum-size notice replaces the layout without discarding the draft.
Growing again restores the body, sidebar and composer.

The reference inspected was `charmbracelet/crush` at revision
`1f3827bcd2d20f38076b2d46123683271e6ed9ba`, from
[Crush](https://github.com/charmbracelet/crush). Its
`internal/ui/styles/styles.go` uses U+2713 for `CheckIcon` and `ToolSuccess`;
`internal/ui/chat/messages.go` reserves two columns for message padding and caps
readable content at 120 columns; `internal/ui/chat/tools.go` uses two-column tool
body padding and distinct pending/success/error icons. Fluzo uses the same thin
check glyph without bold, including beside a bold selection marker/title.
This is not full body parity: Fluzo still uses bounded plain retained lines with
horizontal panning, rather than Crush's width-aware rich Markdown and expandable
tool output. Its decorative startup check remains explicitly labeled Demo running.

On the uncommitted tree based on `9d7f7b6`, 86 Rust and 45 Python tests pass.
The six PTY paths now resize without SIGWINCH from 30x8 through 120x40, 160x50,
240x80 and back to 80x24. They verify composer/draft/status rows at the new height
and the sidebar separator at the new width, not just a possibly stale text marker.
The active-animation PTY path also passed three consecutive focused runs; this
is bounded regression evidence, not proof against every scheduling interleaving.
An earlier resize regression failed because the minimum notice clipped at 30
columns; the compact notice fixed that failure. Earlier active resize/search
runs also exposed intermittent timing failures, retained in the evidence above.
Formatting, workspace check/build, Clippy, dependency/skill checks, the full Python
suite and isolated LSP checks pass. No dependencies or terminal configuration
were changed. These tests establish application geometry handling, not GUI window
manager behavior: dragging the Ghostty window border remains manually unverified.
Ghostty 1.3.1 on KDE reports automatic decorations and no configured initial
width/height; a per-launch server-decoration override is a diagnostic alternative,
not an established fix for the reported drag problem.

## UI-02 part 1: presentation and reversible previews

This is the approved first increment of [UI-02 (#7)](https://github.com/fluzo-labs/fluzo/issues/7),
following PRD 29.1, 29.2.1, 29.2.2 and architecture A06/A07 at the baseline above.
It does not complete the parent story or implement configuration persistence.

```sh
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive --dev-menu
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive --animation-fps 0 --ascii
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive --theme high-contrast --reduced-motion
```

The CLI accepts visual options only after `demo --interactive`. Animation FPS
accepts every integer 0 through 60, default 60; 30 and 15 reduce scheduling work,
0 disables motion. Reduced motion takes precedence. `--no-dev-menu` wins over
`--dev-menu` regardless of ordering. Unknown options, invalid rates and unknown
themes fail before terminal setup without echoing untrusted argument values.
Plain demo/help/version remain nonanimated and do not load project settings.

The user-approved visual study replaces the pixel Y with the compact upright
FLUZO relief wordmark, without a separate symbol. The three-row wordmark sits
below a top-edge activity line occupying 80% of the viewport width. During WORK,
the line travels edge to edge and back in 0.8 seconds, with a separate fast
cyan/blue/violet gradient; the wordmark has a slower gradient. The four-row
allocation never changes. The default theme uses uniform charcoal #201F26,
following the corrected user color selection. Empty wordmark cells and the
activity line inherit that surface; occupied two-color relief cells retain their
artwork colors. NO_COLOR retains terminal defaults; high contrast retains black. Terminal-native
half blocks need no image protocol; `--ascii` changes artwork to ASCII. `TERM=dumb`
and `TERM=linux` select monochrome ASCII artwork automatically (not a claim that
all other widgets or the event backend support every terminal). Default and
high-contrast themes share focus, neutral, success, warning and failure tokens.
Required state labels and NORMAL permission mode remain visible without color.

Ctrl+P exposes Theme preview in ordinary demos. With devmenu enabled, search for
Developer menu. Search matches supported setting keys/purposes; Up/Down selects,
Left/Right changes the selected typed value (FPS steps by one, bounded to 0..60).
The menu shows the canonical key, purpose, default, effective value, provenance
and live-application behavior. `CommandLine` values are locked, including reset.
The core registry supplies theme choices and the FPS ceiling to both validation
and controls; there is no second default configuration.

| Action | Effect |
| --- | --- |
| Ctrl+A | Apply supported preferences to this demo session only |
| Ctrl+R | Preview defaults for unlocked supported preferences |
| Esc | Revert unapplied preferences and synthetic state; restore navigation |
| Ctrl+C | Revert before asking to exit; Esc cancels playback |
| Ctrl+N in devmenu | Advance to the next host-supplied synthetic snapshot |

Editable keys in this increment are `tui.theme`, `tui.animation_fps`,
`tui.reduced_motion` and the boolean `tui.flags.render_diagnostics`.
Theme preview is independent of devmenu enablement. Closing or applying never
persists synthetic states, dispatches commands or changes runtime authority.
Composer draft, focus and retained-line anchor survive previews and stream input.
Applying increments a session UI version and produces a bounded, non-secret
status diagnostic; it does not claim a durable audit/export event.

The CLI supplies validated, owned application snapshots for Running, Waiting
(user input), Blocked (permission), Completed (synthetic verification), Failed,
Cancelled and Pending. These are explicitly isolated scenario data, not real
outcomes or a UI task engine. Only Ctrl+N advances them; rendering, polling,
opening a menu and streaming text cannot advance a task. Leaving the preview
restores the original snapshot even after applying preferences.

Motion derives from elapsed monotonic time, independently of synthetic fragments.
WORK means synthetic stream playback or a Running demo snapshot, not real agent
execution. Ctrl+P > Play synthetic stream starts playback; Esc stops it.
The stabilization contract above supersedes the earlier static Waiting and
terminal-state styling: Waiting pulses and Completed settles after one second.
Reduced motion and FPS 0 disable both movement and color animation. One pending
animation deadline skips obsolete frames, and a final redraw restores static idle.
Static states have no animation deadline. Input polling retains its bounded
50 ms signal-check ceiling and is never throttled by the selected FPS.
Animation-only redraws reuse one viewport-sized Ratatui cell buffer and update the
identity region, avoiding conversation/editor reformatting; full invalidation
occurs on input, resize, theme or snapshot changes. Cached redraws preserve cursor
and modal contents. This is not a competing terminal diff engine.
Rendering diagnostics show session UI version, last frame duration and skipped
deadlines, explicitly not display FPS. RGB uses COLORTERM truecolor/24bit or
TERM=xterm-ghostty; other terminals use basic colors, and NO_COLOR remains honored.
Ghostty uses synchronized updates around Ratatui's differential draw, including
best-effort synchronization termination on failure. No line erase occurs per tick. No frame
emits a domain event, log record, trace span or provider request.

### Remaining UI-02 increment and acceptance

A future persistence increment needs separate authorization and the validated
settings path shared with UI-03 (#8): selected-preference atomic save, conflict/reload handling,
failed-write preservation, saved-versus-session provenance and CLI precedence.
Save is visibly unavailable here, rather than simulating success or touching a
real workspace. No `.fluzo` is read or written in this first increment.

The parent also retains the cited controls that need additional typed contracts
or adapters: bounded pulse/intensity and implemented layout/tool-detail variants,
notification previews/settings, consent-aware Laya status/control, pending-restart
presentation and sanitized configuration diagnostic exports. They are not fake
selectable flags in this increment. No acceptance requirement is removed, and
this work must not close #7. Human artwork/contrast review and reference-terminal
performance remain separate gates. No child issues or Project state were changed
by the local implementation.

The maintainer accepted the separate Ghostty header study (upright relief,
no symbol, transparent background, moving gradient and fast 80%-width activity
line) in conversation before integration. This visual direction differs from the
immutable baseline's Y-logo requirement and still needs reconciliation in the
design repository before parent acceptance; this implementation changes only the
demo, not the baseline or issue criteria.

### Integrated header verification

The approved header integration passes 80 Rust tests and the existing 45-test
Python suite. The six PTY paths include RGB and synchronized-update sequences,
WORK/IDLE playback transitions, resize, cancellation and terminal restoration.
Formatting, workspace build, Clippy, graph checks and isolated LSP checks pass.
A stale LSP symbol range initially broke the edited function; exact source-based
repair restored it before the passing checks. No dependency or lockfile changed.
The integrated Ghostty demo is available for visual confirmation; this does not
claim reference performance or parent acceptance.

### Part 1 verification evidence

On the uncommitted UI-02 part 1 tree based on `9d7f7b6`, 76 Rust tests and 45
Python tests passed, including six PTY tests. Commands run from the repository
root: `cargo test --workspace --locked --offline`, `cargo check --workspace
--all-targets --locked --offline`, `cargo clippy --workspace --all-targets --locked
--offline -- -D warnings`, `cargo fmt --all -- --check`, `cargo build --workspace
--locked --offline`, `python3 scripts/check_dev_setup.py`, `python3 -m unittest
discover -s scripts -p 'test_*.py'`, `python3 scripts/test_http_simulator.py`,
`python3 scripts/check_lsp.py` and `git diff --check`.

Regressions cover exact elapsed-time artwork equivalence at 15/30/60, zero and
reduced motion, idle deadlines, skipped backlog, CLI validation/locking, defaults,
rollback through Esc/cancel/quit, session application, denied nonvisual keys,
validated demo snapshots, bounded menus and cached/full-render equality at
60x16, 80x24, 120x40 and 160x50. PTY paths exercise 0/60 FPS, theme/developer
search, locked values, state changes, resize during activity, paste, stream,
normal/signal exits and byte-identical workspace/configuration preservation.

Initial PTY verification caught a hidden state label under the menu; moving visual
previews below the identity restored visibility. Buffer equivalence caught stale
bold modifiers in cached logo cells; explicit identity-cell reset fixed it. A
cache type-inference compilation error and trailing whitespace were corrected.
All these checks were rerun successfully, not discarded or weakened.

Self-review: no dependency/feature/lockfile changes, no runtime imports in TUI,
no command/provider/file-writing path in preview controls, no operational keys
accepted, and no staged or pre-existing changes overwritten. The graph checker,
negative graph fixtures and offline cross-crate LSP fixture passed. Remote CI,
Ghostty/Alacritty artwork and contrast review, display performance and live
inference are not run for this increment. Local verification is not parent
acceptance, issue closure or publication.

## UI-01 verification and limitations

```sh
cargo test -p fluzo-tui --locked --offline
python3 -m unittest discover -s scripts -p 'test_tui.py'
cargo test --workspace --locked --offline
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_dev_setup.py
python3 scripts/check_lsp.py
```

Local evidence on the uncommitted UI-01 changes over `d7621aa` (whose tree matches
merged SIM-01 `24b5077`): 67 Rust tests and 41 Python tests passed. PTY tests use a
private temporary workspace, cleared environment, synthetic input, bounded waits,
owned child cleanup and a small ANSI screen observer. They exercise safe paste,
palette search, stream start/cancel, resize, ordinary exit, SIGTERM and non-TTY
failure; termios must equal its exact initial state afterwards. Synthetic source
and invalid `.fluzo` contents remain unchanged. Unit tests cover retained focus,
drafts/anchors, safe exit selection, Unicode edits, bounded output, control-string
fragments, viewport buffers and independent cleanup attempts after write errors.

The first PTY assertions searched raw byte strings and failed because Ratatui
rewrites only changed cells. They were replaced by a screen observer, not weakened
product assertions. The first graph extension exposed aliasing of the shared
Serde allowlist; independent set copies and negative regressions fixed it.
Initial offline metadata lacked a conditional Windows artifact; explicit locked
build-dependency preparation completed the cache before offline validation.

Ghostty/Alacritty visual review, snapshots reviewed by the product owner,
screen-reader testing, reference performance, large runtime streaming and full
MVP visual acceptance are not run. PTY/buffer checks do not prove visual polish or
display FPS. No live inference was run. CI still needs to run on the published
revision. There is no issue closure, commit or publication implied by local tests.
