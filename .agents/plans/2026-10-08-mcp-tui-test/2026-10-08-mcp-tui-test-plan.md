---
name: "2026-10-08-mcp-tui-test"
description: "Give the coding agent a live, position-aware way to drive and inspect the Fluzo TUI during development, without weakening the deterministic PTY evidence suite or adding MCP to the Fluzo product."
created_at: "2026-10-08T13:30:55Z"
last_implementation_at: "2026-10-08T14:41:30Z"
has_completed_all_phases: true
---

# Accelerate Fluzo TUI development with a live agent-driven terminal harness

## Source and revision

- Plan identifier: `mcp-tui-test-adoption`
- Source: user request to research and plan incorporating `https://github.com/GeorgePearse/mcp-tui-test` to speed up testing.
- Reviewed revision (consumer): `cef2b32e3bc25ccbaa8badf29b6314a0a5b66494` on `main`, working tree clean and in sync with `origin/main`.
- Reviewed revision (upstream tool): `bbdd9003eaefd0609d86707abdb5d32d1540b25c`, committed 2026-10-06T18:57:30Z.
- Design baseline consulted: [AGENTS.md](../../../AGENTS.md) (dependency boundaries, non-negotiable practices, test pyramid), [SKILLS.md](../../../SKILLS.md), [TUI.md](../../../TUI.md), [CONTRIBUTING.md](../../../CONTRIBUTING.md), the upstream README and docs site, and the four existing PTY harness files under `scripts/`.
- Approval: content and destination explicitly approved by the user on 2026-10-08, together with the three decisions recorded below. This annotation is not proof of authorization by itself.

## Goal

Let the coding agent launch, drive and inspect the real Fluzo TUI in a live terminal session through MCP tools, with true terminal emulation and column-precise position assertions, so visual-layout iteration stops requiring a hand-written PTY test per hypothesis. The existing deterministic unittest PTY suite remains the evidence layer for acceptance.

## Scope and exclusions

In scope:

- A pinned, reproducible developer-time install of `mcp-tui-test` from a fixed upstream commit.
- Registration of that server in the project `crushrc`, plus the matching update to the byte-exact assertion in `scripts/check_dev_setup.py`.
- A demonstrated agent-driven exploration of `fluzo demo --interactive` in buffer mode.
- A written boundary stating what this tool is for, what it is not for, and which layer remains authoritative for acceptance evidence.

Excluded and preserved:

- **No MCP or ACP support inside the Fluzo product.** `AGENTS.md:98` lists ACP/MCP as later work, `SKILLS.md:21` forbids adding MCP/ACP support or automatic skill discovery to Fluzo itself, and `TUI.md:807` forbids fake System/Custom/MCP palette categories. Everything here is developer-time tooling for the coding agent and must never be presented as a Fluzo capability.
- **No CI integration.** The four existing PTY files stay the CI suite and keep their zero-third-party-Python-dependency property. No workflow step installs `uv`, `pexpect`, `pyte` or FastMCP.
- **No replacement of the existing PTY suite.** No deletion, rewrite or migration of `scripts/test_tui.py`, `test_setup.py`, `test_configuration.py` or `test_model_wizard.py`.
- No new Rust code, no new crates, no product dependency changes, no changes to `Cargo.toml` or `Cargo.lock`.
- No pytest adoption. The upstream pytest plugin is noted as an option but is not adopted, because `python3 -m unittest discover -s scripts -p 'test_*.py'` is the discovery command in CI and a pytest suite would not run under it.

## Verified context

Observed facts:

- Upstream is MIT licensed, matching this MIT workspace. 19 stars, 7 forks, 7 open issues, created 2025-11-01, last pushed 2026-10-06. **No git releases exist.**
- Upstream is **not published on PyPI**: `https://pypi.org/pypi/mcp-tui-test/json` returns 404. The README states "Once published to PyPI, no install is needed", so `uvx mcp-tui-test` fails today. A git-based channel is required.
- Python implementation requires Python 3.10+ and `uv`; dependencies are FastMCP (server), pexpect (spawn and control) and pyte (VT emulation for buffer mode).
- A sibling Go implementation in upstream `go/` offers the same tool contract (same tool names, same structured result shapes, same `tui://{session_id}/screen` resource) as a single static binary built on the official Go MCP SDK, `creack/pty` and `vt10x`.
- Local machine: `/usr/bin/uv` and `/usr/bin/uvx` present. **No `go` on PATH.** No `pytest`, `pexpect` or `pyte` installed. `python3` is 3.14.7, so upstream's 3.10+ claim against 3.14 is **unverified**.
- Baseline before this phase: Crush had **no MCP servers configured** (`crush_info` reported no `[mcp]` section). Discovered configuration files are `~/.config/crush/crushrc`, `~/.local/share/crush/crush.json` and `/home/jose/gitrepos/fluzo/crushrc`.
- The project `crushrc` is exactly one line. `scripts/check_dev_setup.py:275-276` asserts its exact bytes:
  `lsp add rust-analyzer --command rustup --args run --args 1.98.0 --args rust-analyzer --filetypes rust --root-markers Cargo.toml\n`
  Adding an MCP entry to that file therefore **breaks `python3 scripts/check_dev_setup.py`**, which CI runs, unless the assertion is updated in the same change.
- Crush MCP syntax: `mcp add <name> --type stdio --command CMD --args ARG [--env KEY VALUE] [--timeout N]`. Project-local configuration takes priority over the user file; both formats are trusted code executed at load with user privileges.
- Tool surface: `launch_tui` (command, session_id, timeout, dimensions such as `80x24`, mode `stream`/`buffer`), `send_keys` (with `delay` defaulting to 0.1s), `send_ctrl`, `capture_screen`, `expect_text`, `assert_contains`, `assert_at_position` (buffer only), `get_cursor_position` (buffer only), `get_screen_region` (buffer only), `get_line` (buffer only), `close_session`, `list_sessions`. Results are structured with a `success` flag plus typed fields (`screen`, `outcome`, `passed`, `row`/`col`, `screen_excerpt` on failed assertions, `error`).
- Session lifecycle is bounded upstream: `MCP_TUI_MAX_SESSIONS` (default 8, structured error past the cap), `MCP_TUI_SESSION_TTL_S` (default 900, closed on next access), dead-process reaping on every registry access, and SIGKILL escalation at interpreter exit.
- Fluzo's current PTY harness is four files totalling 1119 lines: `test_tui.py` (464), `test_setup.py` (226), `test_configuration.py` (262), `test_model_wizard.py` (167). Roughly 330 lines are harness boilerplate, about 641 lines are scripted key/marker pairs (~180 pairs), and about 74 lines are wrappers.
- The only shared code is `class Screen` at `scripts/test_tui.py:20-53`. It hardcodes 90 rows by 260 columns in `__init__` (line 22), unrelated to the actual PTY size; `feed()` (lines 28-50) uses an incremental UTF-8 decoder plus the regex `\x1b\[([0-?]*)([ -/]*)([@-~])` and handles **only** CSI `H`/`f` (cursor position, lines 36-38) and `J` with parameter `2` (lines 39-40), plus `\r`, `\n` and printable cells. **SGR, EL, IL/DL, DECSTBM, alt-screen and every other sequence are silently dropped.** The reset is re-typed literally at `test_tui.py:211` and `:381`.
- Consequence: current assertions depend entirely on ratatui emitting an absolute cursor-position sequence per row, and can pin a **row** but never a **column** (`wait_for(marker, row=n)` at `test_tui.py:103-113`). Column-precise and cursor-position checks are impossible today.
- The whole existing harness uses only the Python standard library (`pty`, `fcntl`, `termios`, `select`), which is why CI needs no Python dependency install.
- CI invokes the suite at `.github/workflows/development.yml:29` as `python3 -m unittest discover -s scripts -p 'test_*.py'`. Any new `test_*.py` file under `scripts/` therefore joins CI automatically.

Hypotheses, not yet verified:

- `pexpect` and `pyte` compatibility with Python 3.14.7 is unconfirmed; P1 must confirm or pin an older interpreter.
- Whether `uvx git+https://github.com/GeorgePearse/mcp-tui-test@<sha>` resolves cleanly for this repo layout is unconfirmed; P1 verifies it and falls back to a local checkout with `uv run` if it does not.
- Whether pyte-backed buffer mode faithfully reproduces ratatui's full output (in particular whether the current harness's CUP-only assumption was hiding real rendering behavior) is unconfirmed; P3 produces the concrete finding.

## Decisions, dependencies, and risks

Approved decisions (user, 2026-10-08):

1. **Registration location: the project `crushrc`.** Chosen for team sharing over a user-level file. Accepted consequence: `scripts/check_dev_setup.py:275-276` must be updated in the same change as the MCP entry, so the byte-exact foundation assertion and the new entry cannot drift apart.
2. **Install channel: `uvx git+https://github.com/GeorgePearse/mcp-tui-test@bbdd9003eaefd0609d86707abdb5d32d1540b25c`.** No repository footprint. Chosen over a local checkout and over the Go binary.
3. **No CI integration.** The tool stays developer-time only.

Rejected alternatives, for the record: user-level `~/.config/crush/crushrc` (keeps the repo byte-clean but is not shared with the team); local checkout plus `uv run` (auditable but manual); the Go binary (identical contract and zero runtime dependencies, but blocked because no Go toolchain is installed).

Pending decisions: none blocking. If P1 finds Python 3.14 incompatible, the fallback is `uv python pin 3.12` for the tool environment, which is a local-only choice and does not affect the consumer repository.

Prerequisites:

- Network access for the one-time `uvx` git fetch and dependency resolution. This is developer-tooling installation, separate from the offline Rust checks, and must not be conflated with them.
- Registry access is required only for P1. P2 and P3 run against the already-installed server.

Risks and mitigations:

- **Upstream has no releases.** Pinning to a commit SHA is mandatory; any future bump is a fresh review, not an automatic upgrade. The SHA and license must be recorded in the plan and in the project documentation so provenance survives.
- **Supply chain and trust.** An MCP server is trusted code executed at load with the user's privileges, and Crush performs shell expansion on `command`, `args` and `env`. Mitigation: pin the SHA, keep the entry minimal and explicit, and review the entry as code in the PR.
- **Foundation contract breakage.** Editing the project `crushrc` without updating `check_dev_setup.py` breaks CI. Mitigation: both edits land in the same commit, and `python3 scripts/check_dev_setup.py` is an explicit acceptance criterion of P2.
- **Timing tension with the deterministic testing policy.** `send_keys` defaults to a 0.1s delay per key, and the tool is inherently time-based, whereas `fluzo-deterministic-testing` requires barriers and acknowledgements over arbitrary sleeps. Mitigation: the P3 boundary states this tool is for exploration and diagnosis, never for acceptance evidence, and never replaces barrier-based regression tests.
- **Dependency weight.** Adopting the tool brings `uv`, `pexpect`, `pyte` and FastMCP into a harness that today has zero third-party Python dependencies. Mitigation: the dependency lives only in the developer's `uvx` cache and never in CI or the consumer repo.
- **Interpretation risk against standing project rules.** A reader could mistake this for MCP support in Fluzo, contradicting `AGENTS.md:98`, `SKILLS.md:21` and `TUI.md:807`. Mitigation: the exclusion is stated in Scope and repeated in the P3 documentation deliverable in the project's own words.
- **Documented upstream limitations.** Unix-like systems only, no mouse support, position assertions only in buffer mode, buffer mode uses more memory. All acceptable for the current Fluzo target (Linux) and current TUI (no mouse use).

## Phases

### P1: A pinned, reproducible developer install with recorded provenance

- Outcome: on a clean developer machine, the `mcp-tui-test` server starts from a fixed upstream commit and answers `list_sessions`, and its provenance (URL, commit SHA, MIT license, install channel, "not a CI dependency, not a Fluzo product feature") is written down where a contributor will find it.
- Owning repository: `fluzo-labs/fluzo`, developer tooling only.
- Dependencies: none. Network access for the one-time fetch.
- Affected contracts: none in the consumer codebase. Creates a developer-facing install recipe and a provenance record. No consumer file becomes a runtime dependency of the tool.
- Acceptance criteria:
  - `uvx git+https://github.com/GeorgePearse/mcp-tui-test@bbdd9003eaefd0609d86707abdb5d32d1540b25c` starts the server and a `list_sessions` call returns a structured success response.
  - If that channel fails to resolve, the fallback (local checkout plus `uv run` / `python server.py`) is verified instead and the chosen channel is recorded.
  - Python 3.14.7 compatibility is either confirmed, or an older interpreter is pinned for the tool environment and the pin is documented.
  - The MIT license is verified present in the resolved source.
  - The `.github/workflows/development.yml` file is unchanged, proving no CI integration.
  - Negative case: `uvx mcp-tui-test` (the PyPI form) is confirmed to fail, so no future contributor is misled by the upstream README.
- Actions:
  - [x] Resolve and start the server from the pinned SHA; record the actual command and output.
  - [x] Confirm or pin the interpreter version; record the result.
  - [x] Verify the MIT license in the resolved source and record the SHA-256 of the pinned commit reference.
  - [x] Record the install recipe and provenance in the project documentation, with the explicit "developer-time only, not CI, not product MCP" statement.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review before another phase.
- Verification: from a shell in the consumer root, run the pinned `uvx` command, then a `list_sessions` call against it; run `git diff --stat .github/workflows/` and confirm it is empty; run `python3 scripts/check_dev_setup.py` and confirm it still passes (P1 must not have touched `crushrc`). Expected evidence: a structured session-list response, an empty workflow diff, and a passing setup check.
- Risks and recovery: if the pinned commit does not build under Python 3.14, pin 3.12 for the tool environment only; if the git channel is unusable, switch to the checkout fallback and update the recorded recipe. Neither recovery touches the consumer repository.

### P2: The agent drives the live Fluzo TUI through the registered MCP server

- Outcome: from inside Crush, with no hand-written Python test, the agent launches `fluzo demo --interactive` in buffer mode at a chosen size, waits for the composer, asserts text at a specific **row and column**, toggles the sidebar with `Ctrl+B`, captures the screen, and closes the session. The project `crushrc` carries the MCP entry and the foundation byte assertion is updated to match.
- Owning repository: `fluzo-labs/fluzo` (project `crushrc` and `scripts/check_dev_setup.py`).
- Dependencies: P1 accepted.
- Affected contracts: the project `crushrc` file content changes from one line to two (the existing `lsp add` line plus a new `mcp add` line). The byte-exact expectation in `scripts/check_dev_setup.py:275-276` changes in lockstep. No Rust or product contract changes.
- Acceptance criteria:
  - `crush_info` reports the `tui-test` MCP server as connected, with its tools visible.
  - The agent completes the scripted live exploration end to end, including at least one `assert_at_position` with a non-zero column, which the current harness cannot express.
  - `python3 scripts/check_dev_setup.py` passes with the new `crushrc` bytes.
  - `python3 -m unittest discover -s scripts -p 'test_*.py'` still passes unchanged, proving the existing PTY suite was not disturbed.
  - Negative case: the exploration closes its sessions and `list_sessions` reports no leaked sessions; the terminal is restored (no leftover raw-mode state in the developer shell).
  - Negative case: launching past `MCP_TUI_MAX_SESSIONS` returns a structured error rather than hanging.
- Actions:
  - [x] Add the `mcp add tui-test --type stdio ...` entry to the project `crushrc` using the pinned channel.
  - [x] Update the expected bytes in `scripts/check_dev_setup.py:275-276` in the same change.
  - [x] Reload the project and confirm the server is connected via `crush_info`. Confirmed after a restart: `tui-test = connected (12 tools, 0 resources)`, loaded from `/home/jose/gitrepos/fluzo/crushrc` with no user-level MCP entry. Crush loads MCP servers at startup only, so the session that edited `crushrc` could not confirm this and a restart was required.
  - [x] Drive the live exploration against `fluzo demo --interactive` in buffer mode and capture the actual tool results.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review before another phase.
- Verification: `crush_info` (MCP section present and ready); the exploration transcript showing a passing row-and-column assertion; `python3 scripts/check_dev_setup.py` passing; `python3 -m unittest discover -s scripts -p 'test_*.py'` passing. Working directory is the consumer root throughout.
- Risks and recovery: if the byte assertion and the `crushrc` drift, CI fails fast and the fix is to re-derive the expectation from the actual file. If the MCP server destabilizes the agent session, the entry can be set with `--disabled true` to keep the configuration while removing the runtime effect. Recovery never requires reverting the Rust workspace.

### P3: A written boundary plus the concrete emulation gap finding

- Outcome: a contributor can read one short section and know exactly when to reach for the live MCP harness and when to write a deterministic PTY test, and the project has a recorded, concrete finding about what the hand-rolled `Screen` silently drops, feeding the existing gap in issue #56.
- Owning repository: `fluzo-labs/fluzo` (project documentation).
- Dependencies: P2 accepted.
- Affected contracts: documentation only. Adds a developer-tooling section stating the tool's purpose, its non-purpose, and that the unittest PTY suite remains authoritative for acceptance evidence.
- Acceptance criteria:
  - The boundary is explicit that this is developer-time tooling and that MCP/ACP remain out of scope for the Fluzo product per `AGENTS.md:98`, `SKILLS.md:21` and `TUI.md:807`, so the two cannot be confused.
  - A concrete side-by-side comparison is recorded: the same Fluzo TUI screen rendered through pyte buffer mode versus through `scripts/test_tui.py:20-53`, naming every sequence class the current `Screen` drops (SGR, EL, IL/DL, DECSTBM, alt-screen) and stating whether any current assertion was relying on the CUP-only behavior.
  - The new capabilities the tool unlocks over the current harness are listed with at least one worked example each: column-precise assertion, cursor position, screen region extraction.
  - The timing caveat is recorded: the 0.1s default per-key delay makes this unsuitable for the barrier-based determinism the project requires for acceptance evidence.
  - The finding is written up so it can be linked from issue #56 without restating the whole analysis.
  - All relative links in the new documentation resolve from their final location.
- Actions:
  - [x] Produce the pyte-versus-`Screen` side-by-side comparison on the real Fluzo TUI and record the dropped-sequence findings.
  - [x] Write the developer-tooling boundary section with purpose, non-purpose, worked examples and the timing caveat.
  - [x] Verify every relative link resolves from the final file location.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review.
- Verification: `python3 scripts/check_dev_setup.py` (its skill-reference link validator and foundation checks still pass); manual review of the comparison output against the live screen; the documentation section reviewed for the explicit product-versus-tooling distinction.
- Risks and recovery: if the comparison shows the current harness was hiding a real rendering issue, that is a finding to file, not something to fix silently inside this plan.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | accepted | `MCP_TUI_TEST.md` records the recipe and provenance. At consumer `cef2b32`: pinned `uvx git+...@bbdd900` completes the MCP handshake (server `tui-test`, protocol `2024-11-05`, 12 tools) and `list_sessions` returns `{"success": true, "sessions": [], "reaped": {}}`; PyPI form fails as expected; uvx selects Python 3.13.16 and a forced `--python 3.14` run reproduces the same handshake on 3.14.7; LICENSE SHA-256 `1d28272e...1e6f46` matches between clean clone and uv cache; `git diff .github/` empty; `python3 scripts/check_dev_setup.py` passes; `python3 -m unittest discover -s scripts -p 'test_*.py'` passes 73 tests. |
| P2 | implemented | passed | accepted | `crushrc` carries the pinned `mcp add tui-test` line and `scripts/check_dev_setup.py` expects those exact bytes (setup check passes). Live drive over MCP stdio against the same pinned server, consumer `cef2b32`: 17/17 checks passed. Buffer-mode launch at 120x40; composer `Ask anything` located at row 34, column 5; `assert_at_position` passes there and fails at (34, 8) and (39, 5), proving row and column sensitivity; cursor reported (34, 5); `get_line` and `get_screen_region` returned as requested; `Ctrl+B` collapses the wide sidebar to the compact status and again restores it; poisoned `.fluzo` never read; `close_session` leaves `sessions: []`; the ninth concurrent launch returns the structured `session cap reached (8); close a session or raise MCP_TUI_MAX_SESSIONS` rather than hanging; no orphaned demo processes after server exit. 73 unittest tests still pass and `.github/` is unchanged. The `crush_info` criterion is verified: after a restart the agent reports `tui-test = connected (12 tools, 0 resources)` loaded from the project `crushrc`, with no user-level MCP entry. |
| P3 | implemented | passed | accepted | `MCP_TUI_TEST.md` gains "Boundary with the deterministic PTY suite", "Where the two genuinely diverge" and "Usage notes". Census of one captured real-demo stream at 120x40: 360 CUP, 45 SGR, 17 DECSET (`?1000h`, `?1002h`, `?1049h`, `?2004h`) and 1 DECTCEM (`?25l`); the repo `Screen` drops the last three classes, and both emulators still produced **identical text** (zero differing rows across palette open, typed query, escape and sidebar toggle), so no existing assertion depended on the drops. That agreement is a property of ratatui's CUP-per-row diff renderer, not a guarantee. EL, IL/DL and DECSTBM were **not emitted** on this path, so the divergence for them was proven with a synthetic differential instead: EL leaves stale cells that `wait_for` can match after a real terminal erased them; IL/DL produce no shift; and the hardcoded 90x260 grid at `scripts/test_tui.py:22` keeps a 150-column line unwrapped where a 120-column terminal wraps it. Also recorded: `expect_text` consumes the pexpect stream in buffer mode (observed blank buffer, so poll `capture_screen` instead) and `launch_tui` needs an `sh -c` wrapper because it spawns argv without a shell. Relative links in `MCP_TUI_TEST.md` and `README.md` all resolve; setup check passes. |

P1 review acceptance is user-supplied, recorded 2026-10-08 from the explicit
directive "dale caña a P1, P2 y P3 vamos a por todo", given after the P1
evidence table was presented. It is not a self-approval and it does not extend
to later phases: P2 and P3 still stop for their own review.

P2 and P3 review acceptance is also user-supplied, recorded 2026-10-08T17:50Z
from the explicit directive "acepto P2 y P3 y registra la aceptación en el
plan", given after both phases' evidence had been presented and after PR #58
was merged into `main` as `c83b43a`. As with P1, this is not a self-approval:
the reviewer is the user, and the merge was authorized separately from the
publication approval that preceded it.

## Next step

Nothing remains to review in this plan. P1, P2 and P3 are implemented, verified
and accepted, and the work is merged into `main` as `c83b43a` through PR #58.

Two things recorded at acceptance, kept here because they shaped what was
actually delivered:

- The P2 criterion "crush_info reports the tui-test MCP server as connected" is **verified**. After a restart the agent reports `tui-test = connected (12 tools, 0 resources)`, loaded from `/home/jose/gitrepos/fluzo/crushrc` with no user-level MCP entry. Crush reads MCP configuration only at startup, which is why the session that edited `crushrc` could not confirm it at the time.
- The P3 comparison came out differently than the plan predicted. The plan expected the side-by-side to name EL, IL/DL and DECSTBM among the sequences the current `Screen` drops on real traffic. The captured demo stream never emits them, so the recorded finding is narrower and stronger: the repo emulator and pyte agree exactly today, the drops that do occur are SGR, DECSET and DECTCEM, and the EL/IL/DL and grid-size gaps were proven with a synthetic differential instead. Nothing was hidden or fixed silently; the divergence is documented as a latent risk tied to ratatui's renderer, not a present bug.

Follow-on work lives in GitHub, not in this document. #56 is unblocked and
directly primed by the P3 finding: the repo's PTY emulator can pin a row but
never a column, and erase-to-end-of-line leaves stale cells that a text match
still hits. That has to be resolved before the restart-pending line can be
asserted reliably there.

## Evidence and tracking

- Consumer revision reviewed: `cef2b32e3bc25ccbaa8badf29b6314a0a5b66494` (`main`, clean).
- Upstream tool reviewed: `bbdd9003eaefd0609d86707abdb5d32d1540b25c` (MIT, 2026-10-06T18:57:30Z, no releases).
- Related consumer issues: #56 (settings restart-pending line coverage, which the P3 column-assertion finding feeds) and #57 (Ghostty/Wayland resize reproduction). GitHub remains the authority for their state; this document is not a parallel board.
- Implementation evidence for P1, P2 and P3 is recorded in the progress table above and in `MCP_TUI_TEST.md`. The work is merged into `main` as `c83b43a` through PR [#58](https://github.com/fluzo-labs/fluzo/pull/58), and the emulator gap finding is posted on [#56](https://github.com/fluzo-labs/fluzo/issues/56). GitHub is the authority for their state.

## Optional handoff to another skill

For an explicitly authorized autonomous run, follow the local
[autonomous contract](../../skills/plan-execute/references/autonomous.md). Carry the
verified grant and approval reference, the operation allowlist, the original
deadline and consumed counters, together with the plan revision and step result.
This plan and its Approval field never authorize themselves.

Transmit as data: the goal, source and both reviewed revisions, the design
baseline, scope and exclusions (especially the "no MCP in the Fluzo product" and
"no CI" exclusions), the owning repository, the selected phase, the affected
`crushrc` and `check_dev_setup.py` contracts, the acceptance criteria, the
dependencies, the verification commands, and the pending or confirmed approval
with its reference. Do not include secrets or any instruction to bypass
permissions. The next agent must revalidate state and authorization; copying the
Approval field is not sufficient.
