---
name: fluzo-rust-boundaries
description: Use when adding crates, dependencies, Cargo features or shared application protocol types in Fluzo.
user-invocable: true
---

# Check Rust boundaries

1. Read AGENTS.md architecture and the owning issue. Production edges are
   runtime -> core, tui -> core, cli -> core/runtime/tui. Core has no external
   I/O or terminal/database dependencies. Do not create future feature crates.
2. Identify which component owns state and which only receives owned/versioned
   protocol values. Reject concrete provider, DB, channel or cancellation handles
   in the TUI application API. A command acknowledgement is not completion.
3. Inspect Cargo manifests and resolve the graph with
   `cargo metadata --format-version 1 --locked --offline`.
4. Run `python3 scripts/check_dev_setup.py` and
   `python3 -m unittest discover -s scripts -p 'test_*.py'`.
   The checker rejects forbidden transitive production paths and bootstrap external
   dependencies. Review deliberate dependency additions before updating that policy.
5. Add a failing regression for changed rules; run narrow Rust tests and workspace
   checks. Test supported feature combinations explicitly if new features are added.
6. Report graph evidence separately from semantic ownership review: a legal graph
   cannot prove that pure code never performs I/O through the standard library.

## Calibration cases

Reject tui -> runtime, runtime -> tui, core -> cli, and indirect paths creating
those couplings. Keep a legal cli -> runtime -> core path. Do not classify a
serializable owned snapshot as an unnecessary clone solely to avoid allocation.
