---
name: rust-review
description: Use for an explicit Rust code review or pre-PR review in Fluzo; report evidence-backed correctness and safety findings.
user-invocable: true
---

# Rust review for Fluzo

Modified adaptation of apollographql/rust-best-practices rust-code-review at
`eb485a5ddb68e0ded3d79549e994f63ec0a6f6c0`, reviewed 2026-09-24.
See [provenance](ORIGIN.md) and [Apache-2.0 license](LICENSE).

1. Read the diff, owning issue, AGENTS.md and relevant callers/tests. Use semantic
   references where available; do not judge a changed function in isolation.
2. Check ownership and allocations: borrow for local reads, use owned DTOs at the
   application port, justify clones in hot paths and prefer clear control flow.
3. Check typed errors and error propagation; reject panic on recoverable input,
   discarded failures, secret-bearing diagnostics and invented success.
4. Check production dependency direction, durability before dispatch, current
   grants/versions, cancellation ownership and uncertain outcomes. Review async
   queue limits and ensure TUI cannot bypass runtime authority.
5. Verify negative tests, independent effect verification, deterministic fixtures,
   and no credentials/live inference in mandatory checks.
6. Run `cargo fmt --all -- --check`,
   `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`,
   `cargo test --workspace --locked --offline`, and
   `python3 scripts/check_dev_setup.py` from the repository root.
7. Report findings by severity with file:line, concrete triggering conditions,
   impact and a targeted fix. Distinguish verified behavior from unrun checks.

## Severity

- P0: memory unsoundness, silent corruption or an authorization bypass.
- P1: correctness failure, lost/duplicated effects, missing error propagation.
- P2: evidenced performance regression, flaky tests or missing contract coverage.
- P3: readability or unnecessary allocation without material correctness impact.

## Local changes from upstream

Removed host-specific alwaysApply/globs and compulsory remote handbook loading.
Replaced the all-feature command with the actual workspace check. No new thiserror,
anyhow or snapshot dependency is implied. Do not impose a one-assertion rule when
several assertions establish one invariant. Do not automatically add comments.
Project scope and accepted safety contracts take precedence over generic advice.
