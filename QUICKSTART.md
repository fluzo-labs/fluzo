# Verified quickstart (maintainer path)

Verified from a pristine clone of `fluzo-labs/fluzo` on **2026-10-15** on
Linux x86_64 with the pinned toolchain. Every command below was executed
verbatim against the fresh clone and succeeded. Outputs are trimmed to the
essentials. This is the documented path the maintainer runs; the gaps found
while walking it are listed at the bottom and fixed in this branch.

## Prerequisites (checked)

- `rustup` selecting the pinned toolchain from `rust-toolchain.toml`:
  `1.98.0-x86_64-unknown-linux-gnu`
- Python 3.11+ (tested on `Python 3.14.7`)
- Network access for the one-time `rustup` install and `cargo fetch --locked`.
  All validation then runs `--offline` against the shared registry in
  `$HOME/.cargo`.

> **Gotcha (found walking the path).** Keep the real `HOME` (or set
> `CARGO_HOME` explicitly) for the offline build. Overriding `HOME` to
> isolate a run also hides the pinned registry, so `cargo ... --offline`
> fails with `no matching package named serde`. Isolate the workspace by
> current directory, not by `HOME`.

## Setup

```sh
git clone https://github.com/fluzo-labs/fluzo.git
cd fluzo
rustup show active-toolchain    # 1.98.0-x86_64-unknown-linux-gnu
cargo fetch --locked           # one-time; needs network
```

## Validation (offline, inference-free)

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo build --workspace --locked --offline
cargo test --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/test_http_simulator.py
python3 scripts/check_lsp.py
```

Verified run, all green:

- `cargo test --workspace` — 207 runtime + 184 tui + 31 + 12 + others, 0 failed
- `python3 -m unittest discover` — 89 tests OK (includes the Linux PTY harness)
- `python3 scripts/test_http_simulator.py` — 12 passed; live inference: `not_run`
- `python3 scripts/check_lsp.py` — cross-crate references/rename and E0308
  diagnostics passed in an isolated offline fixture
- `python3 scripts/check_dev_setup.py` —
  `Resolved foundation graph, toolchain, LSP config and 12 skills checked.`

## User-visible surface

```sh
cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo
```

- `--help` prints the full bootstrap surface (demo, setup, telemetry purge,
  history, visual options).
- `demo` prints the synthetic protocol demo:
  `Fluzo protocol demo: synthetic fixtures only; no agent or tools executed.`

### Welcome / typed configuration

Run the binary from a directory with no `.fluzo`:

```sh
./target/debug/fluzo        # headless: {"code":"configuration_required", ...}; no file created
./target/debug/fluzo init   # headless: same; creating the file needs a terminal
```

In a real terminal the same commands open the offline setup wizard; after
creation `F2` enters the configuration-only shell (`Ctrl+P` settings,
`F1` editor help). These flows are asserted by the deterministic PTY harness
in `scripts/test_setup.py` and `scripts/test_configuration.py`.

### Ctrl+T task demo

```sh
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive
```

`Ctrl+T` opens the synthetic task list. The fixture shows eight tasks across
queued / running / blocked-awaiting-permission (x2) / completed / failed /
stopping / cancelled, two permission scopes (credential `$FLUZO_DEMO_TOKEN`
and workspace-write) and two partial diffs with differing verification.
Overlay reachability and screens are asserted by `scripts/test_tui.py`
(`test_ui04_task_permission_and_diff_overlays_at_minimum_size` and
`..._at_desktop_size`); the fixture's state coverage by the Rust unit tests
in `crates/fluzo-tui/src/tasks.rs`.

## What is not implemented

No agent execution, providers, session persistence or measured runtime
performance. The demo and setup flows are synthetic and configuration-only.

## Gaps found while walking the documented path (fixed in this branch)

1. The README Development list omitted `cargo check`, `cargo build` and
   `python3 scripts/test_http_simulator.py`, all of which `AGENTS.md` lists
   as verified foundation commands. Added so the README is self-consistent.
2. The README described the welcome/setup and typed-configuration flows only
   in prose, with no runnable example. This file adds the exact commands.
3. The Ctrl+T multi-state task demo was not mentioned in the README. Added.
4. The offline-registry / `HOME` interaction was undocumented. Added above.
5. The PTY test modules use bare-name imports (`from test_tui import Screen`,
   `import test_setup`) that resolve only under
   `python3 -m unittest discover -s scripts`, not via dotted `scripts.test_*`
   paths. The documented `discover` command works; noted so a first-time user
   does not try dotted paths.
