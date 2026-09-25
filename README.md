# Fluzo

> A lightweight, local-first agentic development runtime written in Rust.

**Status: development bootstrap.** The four-crate Cargo workspace, bootstrap
help/version binary and developer tooling are available. Agent execution, the
TUI, providers and persistence are not implemented. There are no releases or
measured runtime performance results yet.

- [Documentation site](https://fluzo-labs.github.io/fluzo-docs/)
- [Product requirements](https://github.com/fluzo-labs/fluzo-docs/blob/main/PRD.md)
- [Architecture](https://github.com/fluzo-labs/fluzo-docs/blob/main/ARCHITECTURE.md)
- [Organization and backlog](https://github.com/fluzo-labs/fluzo-docs/blob/main/GITHUB_ORGANIZATION.md)
- [Implementation issues](https://github.com/fluzo-labs/fluzo/issues)

## Planned MVP

- Rust CLI/TUI with four crates: `fluzo-core`, `fluzo-runtime`, `fluzo-tui`, `fluzo-cli`.
- Linux x86_64: Arch Linux and CachyOS, validated with Ghostty and Alacritty.
- A native coding agent with explicit permissions, budgets and cancellation.
- Repository-local settings/history, recovery and OpenTelemetry GenAI diagnostics.
- Optional Laya observations; active routing and third-party extensions are post-MVP.
- Deterministic HTTP-provider fixtures so CI and packaging need no real inference.

## Development

Rust 1.98.0 is pinned with rust-analyzer, rust-src, Clippy and rustfmt. Initial
rustup preparation requires network access; the dependency-free bootstrap checks
then run offline. Python 3.11+ is required for development checks.

```sh
rustup show active-toolchain
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_lsp.py
cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help
```

The binary does not execute tasks. Foundation checks cannot establish runtime
acceptance. See [AGENTS.md](AGENTS.md) for boundaries and verified commands and
[SKILLS.md](SKILLS.md) for installed skills, LSP setup and deferred adoption.
Runtime tests, benchmarks and release packaging will be documented when implemented.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
and [SECURITY.md](SECURITY.md). Do not commit private settings, credentials, model
weights or `.fluzodrive/` history. Changes to agreed scope require an explicit
design review in the documentation repository.

Original project content is [MIT licensed](LICENSE), copyright 2026 Jose Corral.