# Interactive shell prototype

UI-01 ([#6](https://github.com/fluzo-labs/fluzo/issues/6)) covers the initial
terminal shell, multiline composer and searchable command palette. References:
PRD 29/30/31 and architecture A07, section 12, at baseline
`60c5b0732fb710cdf705476cee8d9156a5ecd971`.

## Run and interaction contract

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
Quit demo. Playback supplies presentation text only, stops after 80 fragments and
never claims a task completed. It is not a second execution engine or a replacement
for future provider/application updates. A Cancel key here means stop preview,
not proof of remote cancellation. Session/task selection, authorization dialogs,
force stop, tools, settings editing and durable history are not implemented.

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
panning. Color is ANSI cyan with text-based focus/status, disabled for nonempty
`NO_COLOR`. No special font, emoji, truecolor, clipboard or hyperlink support is
required. Original art, themes and animation belong to UI-02, not this shell.

One coordinated writer redraws only dirty state; there is no idle animation loop.
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

## Verification and limitations

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
