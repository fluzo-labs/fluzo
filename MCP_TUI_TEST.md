# Live TUI exploration harness

Developer-time tooling for driving the real Fluzo TUI from the coding agent.
This is **not** a Fluzo feature.

## What this is and is not

`mcp-tui-test` is an MCP server the developer registers with their coding agent
so the agent can launch, drive and inspect a terminal application in a live
session. It exists to speed up visual-layout iteration during development.

It is explicitly **not** any of the following:

- **Not MCP support in the Fluzo product.** ACP/MCP remain later work
  ([AGENTS.md](AGENTS.md)) and adding MCP or ACP support, runtime plugins or
  automatic skill discovery to Fluzo itself is out of scope
  ([SKILLS.md](SKILLS.md)). Fluzo contains no MCP client, server or protocol
  code because of this. The agent talks to the harness; Fluzo knows nothing about
  it. The command palette likewise gains no System/Custom/MCP categories
  ([TUI.md](TUI.md)).
- **Not a CI dependency.** CI never installs `uv`, `pexpect`, `pyte` or FastMCP.
  The deterministic unittest PTY suite under `scripts/` remains the acceptance
  evidence layer and keeps its zero-third-party-Python-dependency property.
- **Not an acceptance evidence source.** Its per-key delay and time-based model
  do not meet the barrier-based determinism the project requires for acceptance.
  Use it to explore and diagnose; write the durable test in `scripts/`.

## Install recipe

The upstream package is not published on PyPI, so the channel is a pinned git
reference:

```sh
uvx git+https://github.com/GeorgePearse/mcp-tui-test@bbdd9003eaefd0609d86707abdb5d32d1540b25c mcp-tui-test
```

`uv` resolves an isolated tool environment; nothing is added to this repository
or to the system Python. The upstream README advertises `uvx mcp-tui-test`,
which **fails** today because the package is absent from the registry. Always use
the pinned git form.

Any future revision bump is a fresh review, not an automatic upgrade. Re-verify
the license and re-run the checks below before changing the SHA.

## Provenance

| Field | Value |
| --- | --- |
| Upstream | `https://github.com/GeorgePearse/mcp-tui-test` |
| Pinned commit | `bbdd9003eaefd0609d86707abdb5d32d1540b25c` |
| Commit date | 2026-10-06T18:57:30Z |
| Package version | 0.3.0 |
| License | MIT |
| LICENSE SHA-256 | `1d28272ec16276e444369e8806b6cef5718362fce9d78548b25262a61a1e6f46` |
| Release tags | None upstream; pinning to a commit is mandatory |
| Resolved dependencies | `mcp` 2.3.0, `pexpect` 4.9.0, `pyte` 0.8.2, `typing-extensions` 4.16.0 |

The LICENSE SHA-256 above was compared byte-for-byte between a clean clone at the
pinned commit and the file in the `uv` resolved cache; they match.

An MCP server is trusted code that runs with the developer's privileges at load
time, and Crush performs shell expansion on `command`, `args` and `env`. Review
the registration entry as code.

## Verified commands

Recorded at consumer revision `cef2b32e3bc25ccbaa8badf29b6314a0a5b66494`.

| Check | Result |
| --- | --- |
| `uvx mcp-tui-test` (PyPI form) | Fails: "not found in the package registry" |
| Pinned git channel, MCP handshake | Server `tui-test` initializes, protocol `2024-11-05` |
| `tools/list` | 12 tools exposed |
| `tools/call list_sessions` | `{"success": true, "sessions": [], "reaped": {}}` |
| Interpreter uvx selects by default | Python 3.13.16 |
| Forced `--python 3.14` end to end | Same handshake and tool list on Python 3.14.7 |
| `.github/workflows/development.yml` | Unchanged |

Driving the actual Fluzo TUI through this harness is recorded below.

### Live drive

`fluzo demo --interactive` at 120x40 in buffer mode, with `HOME` and
`XDG_CONFIG_HOME` pointed at a temporary directory holding a poisoned `.fluzo`,
so the demo provably reads no repository configuration.

| Check | Result |
| --- | --- |
| `launch_tui` buffer 120x40 | `success`, width 120, height 40 |
| Composer renders | `Ask anything` located at row 34, column 5 |
| `assert_at_position` at the located (34, 5) | `passed: true` |
| `assert_at_position` at (34, 8) | `passed: false`, found `"anything… ("` |
| `assert_at_position` at (39, 5) | `passed: false`, found `""` |
| `get_cursor_position` | (34, 5) |
| `get_line` row 34 | `"   > Ask anything… (offline demo)"` |
| `get_screen_region` rows 0-2, cols 0-39 | Returned as requested |
| `send_ctrl b` collapses the wide sidebar | Compact status appears, sidebar text disappears |
| `send_ctrl b` again restores it | Wide sidebar returns |
| Poisoned `.fluzo` never read | No configuration error on screen |
| `close_session` then `list_sessions` | `sessions: []`, no leak |
| Ninth concurrent launch | Structured error: `session cap reached (8); close a session or raise MCP_TUI_MAX_SESSIONS` |
| Orphaned demo processes after server exit | None |

The two negative position assertions matter: they show the check is genuinely
column- and row-sensitive rather than trivially satisfied.

Registration in this project's `crushrc` takes effect when Crush starts. A
running Crush session does not pick up a mid-session edit, so the live drive
above was performed by speaking MCP over stdio against the same pinned server,
which is the identical transport and tool path. After a restart, `crush_info`
reports `tui-test = connected (12 tools, 0 resources)`, loaded from this
project's `crushrc` with no user-level MCP entry.

## Boundary with the deterministic PTY suite

The two layers answer different questions and neither replaces the other.

| | `scripts/test_*.py` PTY suite | This harness |
| --- | --- | --- |
| Runs in | CI, every push | Developer machine, on demand |
| Python deps | Standard library only | `pexpect`, `pyte`, `mcp` via `uvx` |
| Timing model | Bounded deadlines, barrier style | Per-key `delay` (default 0.1s) |
| Output | Deterministic evidence | Exploration and diagnosis |
| Acceptance | Authoritative | Never |

### What the repo emulator actually drops

`Screen` in `scripts/test_tui.py` understands CSI `H`/`f` (cursor position),
CSI `J` with parameter `2`, `\r`, `\n` and printable cells. Everything else is
consumed and ignored. Feeding one captured stream from the real demo at 120x40
through both it and a `pyte` buffer gives this census:

| Sequence class | Occurrences | Repo `Screen` |
| --- | --- | --- |
| CSI `H` (CUP) | 360 | Handled |
| CSI `m` (SGR) | 45 | Dropped (harmless for text) |
| CSI `h` (DECSET: `?1000h`, `?1002h`, `?1049h`, `?2004h`) | 17 | Dropped |
| CSI `l` (DECTCEM `?25l`) | 1 | Dropped |

For that stream the two emulators produced **identical text**: zero differing
rows across palette open, typed query, escape and sidebar toggle. That is not
the harness being robust; it is ratatui's diff renderer emitting an absolute
CUP per drawn row and avoiding line-editing sequences on this path. The
agreement is a property of today's renderer, not a guarantee.

### Where the two genuinely diverge

Synthetic streams isolate three concrete gaps in the repo emulator:

| Stream | Repo `Screen` | `pyte` |
| --- | --- | --- |
| `ABCDEF` + home + `ESC[K` (erase to end of line) | Row keeps `ABCDEF` | Row cleared |
| `ESC[1L` / `ESC[1M` (insert/delete line) | No shift | Content shifts |
| 150 columns with no CUP, real terminal 120 | Row is 150 wide | Wraps at 120 |

The first two are false-positive risks: `wait_for` can match text a real
terminal has already erased. The third comes from the hardcoded 90x260 grid at
`scripts/test_tui.py:22`, which is unrelated to the actual PTY size, so
content a real terminal would wrap is kept unwrapped and assertions can pass on
a layout no user would ever see.

Separately, `wait_for(marker, row=n)` can pin a row but never a column, so
today's suite cannot express "the composer placeholder starts at column 5" at
all. That capability exists here and is what makes the position checks above
possible.

These findings feed issue #56 rather than restating it; link this section from
there.

## Usage notes

- **Never use `expect_text` in buffer mode.** It calls `pexpect.expect`, which
  consumes the matched bytes into `before`, so those bytes never reach the
  `pyte` stream. Observed effect: the screen buffer went blank and a later
  `capture_screen` returned an empty grid. Poll `capture_screen` instead.
  `expect_text` is sound only in stream mode.
- **`launch_tui` spawns argv directly, with no shell.** A `cd` prefix fails with
  `The command was not found or was not executable: cd.`. There is no `cwd`
  parameter, so wrap the command: `sh -c '<cd dir> && exec env ... <binary>'`.
- **`send_keys` defaults to a 0.1s per-key delay.** Fine for exploration, wrong
  for anything that needs to be reproducible.
- **Sessions are a shared, capped resource.** Close them explicitly; the cap is
  per server process, not per repository.

## Tool surface

`launch_tui`, `send_keys`, `send_ctrl`, `capture_screen`, `expect_text`,
`assert_contains`, `assert_at_position`, `get_cursor_position`,
`get_screen_region`, `get_line`, `close_session`, `list_sessions`.

Results are structured, with a `success` flag plus typed fields, so the agent
branches on fields rather than parsing prose. Buffer mode adds position, cursor
and region access. Sessions are bounded upstream: `MCP_TUI_MAX_SESSIONS`
(default 8) and `MCP_TUI_SESSION_TTL_S` (default 900), with dead-process
reaping on registry access.

Upstream limits: Unix-like systems only, no mouse support, position assertions
only in buffer mode.
