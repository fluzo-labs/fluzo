---
name: rust-practices
description: Use when implementing or refactoring Rust ownership, typed errors, bounded async work or performance-sensitive code in Fluzo.
user-invocable: true
---

# Rust practices for Fluzo

Adapted on 2026-09-24 from selected leonardomso/rust-skills rules at
`fd2a861ab0406a4ac536a55274d14ea6fd1ca9c9`. This is a modified, deliberately
small selection, not the complete upstream skill. See [provenance](ORIGIN.md)
and [MIT license](LICENSE).

1. Read the owning issue and relevant architecture contract in the root AGENTS.md.
2. Use definition/reference tools before changing shared code. If LSP is unavailable,
   use source search and record that semantic verification was unavailable.
3. Read only the applicable section of [selected rules](references/selected.md).
4. Preserve owned application-port DTOs, typed domain errors and crate boundaries.
   Do not add dependencies, feature flags or change the pinned toolchain from examples.
5. Add a focused success/error regression, run it, then run the checks documented
   in AGENTS.md. Report actual results, not hypothetical passing commands.

## Fluzo-specific overrides

No panic or silent default fallback on invalid user configuration. No generic
retry/failover policy: 429/capacity rejections and uncertain side effects never
retry automatically. No unconditional permit release after network dispatch.
Count and byte bounds are both needed; a bounded channel is not durable audit.
Input/control and diagnostic producers must not await an indefinitely full queue.
The workspace forbids unsafe code; optimization alone does not justify weakening it.
