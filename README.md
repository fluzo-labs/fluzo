# Fluzo

> A lightweight, local-first agentic development runtime written in Rust.

**Status: development bootstrap.** The four-crate Cargo workspace, bootstrap
help/version binary, typed configuration, application protocol and deterministic
scenario driver, interactive demo shell, offline setup and typed configuration
views are available.
Agent execution, full TUI workflows, providers and session persistence are not implemented. There are no releases or
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
rustup and locked dependency preparation require network access on a fresh machine;
validation then runs offline without inference. Python 3.11+ is required for tooling.

```sh
rustup show active-toolchain
cargo fetch --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/check_lsp.py
cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help
cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo
```

The binary does not execute tasks. Foundation checks cannot establish runtime
acceptance. See [AGENTS.md](AGENTS.md) for boundaries and verified commands and
[SKILLS.md](SKILLS.md) for installed skills, LSP setup and deferred adoption.
See [CONFIGURATION.md](CONFIGURATION.md) for the FND-02 library APIs, shared defaults,
validation, dependency policy and scope. `fluzo` opens offline setup when configuration
is missing; `fluzo init` explicitly opens setup, and `--config` selects a workspace-scoped
file. Creation requires review and confirmation. Fluzo owns `.fluzo` and replaces
it when you save, trusting Git instead of a backup copy; when the file changes
outside Fluzo a banner offers Ctrl+R reload or Ctrl+K keep-ours.
Headless missing-config startup returns structured `configuration_required` on stderr.
With valid configuration, interactive startup opens the normal configuration-only
shell: Ctrl+P opens settings without devmenu, F1 shows editing help. After ordinary
setup creation, F2 explicitly enters that shell. Presentation Apply is available;
Save writes the file directly and rebases onto any outside version so unrelated
keys survive. The explicit demo remains
isolated. No provider is contacted. Production active runtime application remains unavailable.
Runtime tests, benchmarks and
release packaging will be documented when implemented. [APPLICATION.md](APPLICATION.md)
describes the FND-03 port, snapshot/cursor contracts and effect-free protocol demo;
it is not an interactive TUI or a production transport.
[HTTP_SIMULATOR.md](HTTP_SIMULATOR.md) describes SIM-01's strict HTTP/SSE fixtures,
test-only dependencies and Linux private-network profile:
`python3 scripts/test_http_simulator.py`. This exercises protocol simulation,
not a native agent repair or live inference.

[UI-01's interactive shell](TUI.md) is available with
`cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo --interactive`.
It provides a multiline composer, searchable palette and synthetic streaming,
without reading configuration or executing tasks. Ctrl+P opens actions;
Ctrl+Q exits with confirmation for a nonempty draft.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
and [SECURITY.md](SECURITY.md). Do not commit private settings, credentials, model
weights or `.fluzodrive/` history. Changes to agreed scope require an explicit
design review in the documentation repository.

Original project content is [MIT licensed](LICENSE), copyright 2026 Jose Corral.