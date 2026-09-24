# Fluzo

> A lightweight, local-first agentic development runtime written in Rust.

**Status: pre-implementation.** The MVP scope and architecture decisions are
approved. This repository currently contains project metadata and the delivery
backlog, not an executable agent. There are no releases or measured performance
results yet.

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

The Cargo workspace is tracked by the foundation backlog. Installation, runtime
tests, benchmarks and release packaging will be documented when implemented.
The initial metadata workflow does not validate a runtime that does not yet exist.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
and [SECURITY.md](SECURITY.md). Do not commit private settings, credentials, model
weights or `.fluzodrive/` history. Changes to agreed scope require an explicit
design review in the documentation repository.

Original project content is [MIT licensed](LICENSE), copyright 2026 Jose Corral.