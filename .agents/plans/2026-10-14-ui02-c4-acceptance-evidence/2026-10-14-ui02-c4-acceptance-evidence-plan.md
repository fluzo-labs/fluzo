---
name: "2026-10-14-ui02-c4-acceptance-evidence"
description: "UI-02 C4 turns the delivered UI-02 slices into one reviewable requirement-to-implementation-to-test-to-evidence matrix with durable sanitized artifacts, honest evidence-class separation and an explicit needs-human list, without claiming runtime, release-performance or cross-view acceptance."
created_at: "2026-10-14T18:25:00Z"
has_completed_all_phases: false
---

# C4: UI-02 acceptance evidence

## Source and revision

- Plan identifier: `ui02-c4-acceptance-evidence` (UI-02 R1 / C4).
- Source: [fluzo-labs/fluzo#39](https://github.com/fluzo-labs/fluzo/issues/39),
  parent [#7](https://github.com/fluzo-labs/fluzo/issues/7). The issue acceptance
  checklist is the contract.
- Branch: `feat/ui02-acceptance-evidence` created from `main` at
  `40577a5f48627474eea53d0b78143c20e226a3ed` (contains merged C3 #67 and the
  P2 configuration evidence #66; C1 #36 and C2 #37 accepted; D0 approved).
- Design baseline: PRD 29.1/29.2.1/29.2.2, PRD 29.4/49 verification boundaries
  and architecture A06/A07 at `fluzo-docs` revision
  `60c5b0732fb710cdf705476cee8d9156a5ecd971`, plus the approved D0 immutable
  revision. Consulted through the issue text and `AGENTS.md`; nothing here amends
  the pinned PRD.

## Current state at this baseline (verified, not assumed)

- 272 Rust tests pass in the workspace: 150 in `fluzo-tui`, 78 runtime, 16 core,
  12 HTTP-simulator integration, 14 CLI (2 binary + 4 bootstrap + 10
  configuration route). 74 Python tests pass, including the PTY suite.
- Buffer/unit coverage already exists at 60x16, 80x24, 120x40, 160x50, 196x36 and
  40x10 viewports. The PTY suite drives 24x80, 16x60, 40x120, 50x160, 240x80 and
  the unsupported 8x30 with its explicit `Minimum: 60x16` notice.
- Color-mode handling is env-driven in `crates/fluzo-tui/src/terminal.rs`
  (`NO_COLOR`, `COLORTERM=truecolor`), and the PTY suite already exercises
  `xterm-ghostty`+truecolor, `xterm-256color` and `NO_COLOR=1`.
- C1/C2/C3 controls, notification isolation and the shared save/apply route are
  implemented and tested; C3 route evidence merged in #67.
- Prior UI-02 evidence in `TUI.md` records relay probes in Ghostty 1.3.1-arch2.2
  and Alacritty 0.17.0 where input went into an inner PTY, not through real
  emulator keyboard events, and where the artifacts stayed in `/tmp`. That is the
  durability gap this slice addresses.

## Gap this slice closes

Nothing was missing in the implementation; what was missing is a single durable,
reviewable evidence object. Specifically: no requirement-to-implementation-to-test
to-evidence matrix existed; no committed sanitized artifact tied to a source
revision existed; the evidence classes (unit/buffer vs PTY vs actual emulator
input vs human review vs timing) were not separated anywhere; and the items that
genuinely need maintainer eyes were scattered across prose.

## Scope (single phase P1)

1. `UI02_ACCEPTANCE_EVIDENCE.md`: the matrix. Every UI-02 parent criterion and
   every approved R1 controls/preview/persistence/isolation contract mapped to
   implementation location, named tests, evidence class and status. Explicit
   `needs human` list. Preserved failed attempts. Exploratory timing findings kept
   as findings. Gaps mapped to owning issues.
2. `scripts/ui02_evidence.py`: a durable fixture generator. It reuses the
   existing `Screen` renderer from `scripts/test_tui.py` (no new framework),
   drives the real binary in a private PTY at 80x24, 120x40 and 160x50 in
   truecolor, ANSI and NO_COLOR plus high-contrast, and writes only the rendered
   cell grid. It refuses to write if any escape byte survives rendering. It waits
   for the screen to stop changing before dumping, so a screen that never becomes
   static at `--animation-fps 0` fails the run instead of producing a blurry
   artifact. `--verify` regenerates and compares against the committed dumps.
3. `evidence/ui02/*.txt`: 15 sanitized committed dumps tied to the source
   revision in each header, with a body SHA-256.
4. `TUI.md`: a pointer section so the prototype document leads to the matrix
   instead of restating it.

Excluded: new implementation code, new test frameworks, image or video capture,
CI artifact hosting, real emulator keyboard injection, runtime integration,
release performance profiling, cross-view review, and any issue closure.

## Acceptance criteria mapping

See `UI02_ACCEPTANCE_EVIDENCE.md` for the row-by-row mapping. Summary of how each
checklist item is answered:

1. Matrix built over integrated behavior: the matrix cites tests that drive
   `Workspace`/`Shell` public input and the CLI route tests that drive the real
   service, not child-issue states.
2. Logo states, 0/15/30/60 FPS, reduced motion, immediate static updates,
   cached/full equivalence and idle: existing controllable-clock unit tests plus
   the new settle requirement in the artifact harness, which is direct evidence
   that the FPS 0 presentation reaches a static frame.
3. Keyboard-only, safe paste, focus/draft/scroll, preview rollback, notifications,
   save/apply failures at the three viewports, resize during activity,
   truecolor/ANSI/NO_COLOR and unsupported sizes: named unit and PTY tests, plus
   the committed dumps for the viewport/color-mode grid.
4. Ghostty/Alacritty environment records and maintainer review: environment
   recorded from this machine for both terminals; every visual-quality judgement is
   placed in the needs-human section rather than inferred.
5. Durable sanitized evidence: committed generator plus committed dumps with
   revision and digest; no temp paths, no raw streams.
6. Evidence classes separated and failed attempts preserved: dedicated sections.
7. Failures resolved or follow-ups recorded with owning issues: gap table.
8. `TUI.md` updated and the matrix linked from the delivery PR.

## Verification commands

Pinned toolchain, `--locked --offline`, from the worktree root:

- `cargo fmt --all -- --check`
- `cargo test --workspace --locked --offline`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- `python3 -B scripts/ui02_evidence.py` then `python3 -B scripts/ui02_evidence.py --verify`

Live inference: `not_run`. No provider listener exists in these fixtures.

## Progress

| Phase | Implemented | Verification | Review | Notes |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | fmt clean; 272 Rust tests pass; 74 Python tests pass; dev-setup clean; 15/15 artifacts regenerate byte-identical under `--verify`; no implementation code changed; live inference not_run; maintainer visual review pending |
