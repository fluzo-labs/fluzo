# T35: AGT-02 scoped context, explicit skills and automatic compaction — plan

Status: approved-for-execution via task handoff (/tmp/fluzo-task-T35.md).
Ref: #21. PRD 16, 16.1, 20 and Architecture A06/A11 at fluzo-docs revision
`60c5b0732fb710cdf705476cee8d9156a5ecd971`.

Ground truth this builds on: AGT-01 gateway (#114, `providers::gateway`),
RUN-01 confinement (#99, `confinement::WorkspaceJail`), RUN-03 closed-set Git
inspection (#111, `git_inspect::Inspection`), RUN-05 budgets/admission (#112,
`RequestClass::Compaction`, `TaskBudgets`, `BudgetConsumption`), DAT-01 store
(#96, `commit_unit`), OBS-01 telemetry (#89).

## Contract

Two layers, matching the pure-rules/live-wiring split already used by
`admission`:

- `fluzo-core::context` — the context ledger as pure data and rules: trust
  tiers, source provenance, token *estimates* distinct from reported usage,
  the effective-input budget arithmetic, deterministic assembly with recorded
  omissions, and explicit-only skill resolution. No I/O, no clock, no
  provider.
- `fluzo-runtime::context` — discovery from the confined workspace and the
  compaction engine that calls the AGT-01 gateway and persists a versioned
  artifact through the store.

Acceptance mapping (issue #21):

- **AC1 "Context is bounded/provenanced"** — every `ContextItem` carries a
  `SourceRef` (kind, identity, observed version) and a `TrustTier`; the pack
  is assembled against a `ContextBudget` whose effective input is the
  configured window minus output, protocol and safety reserves; nothing that
  does not fit is dropped silently — omissions are recorded with their
  provenance and reason.
- **AC2 "summaries preserve source links without becoming authority"** —
  a summary is a `ModelText` item whose `SourceRef` names the exact message
  range it summarizes; `TrustTier::can_authorize()` is false for every tier
  except `RuntimeAuthority`, and the runtime state that must not be
  reconstructed from summary text (budgets, approvals, YOLO, task state,
  workspace versions) is never read out of a summary.
- **AC3 "failed or oversized compaction blocks safely and remains
  observable"** — every non-success path returns a typed
  `CompactionBlocked { reason }`, leaves the previously active context
  pointer untouched, and emits one telemetry record carrying reason, size
  estimates, actual usage when reported, provider/model, duration, artifact
  version, source range and outcome, with no raw content.
- **AC4** — evidence and docs linked from the PR.

Key rules carried from the design baseline:

- Deterministic and testable only. Relevance is a fixed recipe (request +
  assigned workspace + Git status + applicable nested `AGENTS.md` +
  explicitly selected skills), not a score. Same inputs, same bytes out.
- Skills load only through explicit selection (PRD 16, 20). The module
  exposes no function that maps task text to skills; resolution takes ids.
  An empty request selects nothing.
- Automatic discovery considers **tracked** files only, which respects Git
  ignore by construction, plus a closed documented secret-name list that is
  excluded even when tracked. Ignoring is not a security boundary: explicit
  reads still need the file/credential policy.
- Provider-advertised limits are capabilities to validate, not proof. A
  missing configured window is a typed refusal, never "unlimited".
- Estimates never masquerade as usage: `TokenEstimate` and `GenAiUsage` are
  different types and never substituted for one another.
- Compaction runs at a safe boundary, dispatches no tools, mutates no
  workspace, uses exactly one generative provider (no implicit fallback),
  and never routes through the Laya/decision path.
- Oversized history is processed in bounded segments, never as one
  overflowing request. Segments never split a tool call from its result.
- Artifact + source references commit atomically *before* the active pointer
  switches. Recovery selects only a completed artifact and still needs an
  explicit resume.
- Failure preserves the prior context and blocks with a distinct reason:
  provider failure, invalid summary, budget exhausted, cancellation,
  persistence failure, still-does-not-fit, segment budget exhausted. No
  silent discard, no "continue as if it worked".

## Phases

### P1 — context ledger and explicit skills (`fluzo-core::context`)

Trust tiers with an explicit authorization capability; `SourceRef` with
observed versions; `TokenEstimate` separate from reported usage;
`ContextBudget` with `effective_input()` and config validation that refuses
a window whose reserves consume it; deterministic `assemble()` producing
included items plus recorded omissions; `resolve_explicit()` for skills with
unknown/duplicate typed errors and no implicit path. Unit tests.

### P2 — workspace discovery (`fluzo-runtime::context`)

Deterministic recipe over the confined workspace: user request, Git status
and staged/unstaged diff summary at the observed revision, nested
`AGENTS.md` files for the target subtree (root-outward order, each applying
inside its own subtree), and explicitly selected skills. Tracked-only
enumeration, closed secret-name exclusion, per-file byte ceiling with marked
truncation, and a typed refusal when a required item cannot be read. Tests
run against real temporary Git repositories.

### P3 — compaction engine + simulator

`Compactor`: eligibility selection (current request, applicable
instructions, recent exchanges and outstanding work retained), bounded
segmentation that never splits a tool-call/result pair, one gateway call per
segment under `RequestClass::Compaction` with the task budgets and
remaining time, summary structure validation including that every cited
source range lies inside what was actually sent, atomic artifact commit
then pointer switch, and the blocked-reason matrix. Telemetry per PRD 16.1.
Simulator tests on the hardened loopback fixture (#81/#94): successful
compaction and continued execution; oversized input segmented rather than
overflowed; invalid summary; provider failure; cancellation; persistence
interruption; budget exhaustion; permissions/budgets unchanged except
recorded consumption; source links survive recovery.

### P4 — docs

`CONTEXT.md` (contract, trust model, budget arithmetic, compaction
semantics, verified commands, gaps), `AGENTS.md` AGT-02 paragraph.

## Verification (commands to run and record)

- `cargo test -p fluzo-core --lib --locked --offline context`
- `cargo test -p fluzo-runtime --lib --locked --offline context`
- `cargo test -p fluzo-runtime --test context_compaction_simulator --locked --offline`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `cargo test --workspace --locked --offline`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- Live inference: `not_run`

## Non-goals

No agent loop or harness wiring, no TUI/CLI compaction surface, no Laya
summary generation, no cross-instance coordination, no provider fallback,
no semantic/fuzzy relevance, no automatic skill discovery, no lossless-summary
claim, no new dependencies.

## Progress

All four phases implemented and verified. Branch `feat/agt02-scoped-context`.

### P1 - pure context ledger (`fluzo-core`)

- `crates/fluzo-core/src/context.rs`: `TrustTier` (5 levels, only
  `RuntimeAuthority` can authorize), `SourceKind` (11 variants incl.
  `RuntimeRecord`), `SourceRef` (kind + identity + observed version),
  `TokenEstimate` (distinct from `GenAiUsage`, records its own method),
  `ContextBudget` (+ `validate`), `ContextItem` (+ `truncated_at`),
  `ContextPack` (included / omitted / estimated_input), `assemble` with a
  fixed total order, `SkillDescriptor`, `ExplicitSkillSelection`,
  `resolve_explicit`.
- 28 tests. Commit `e347cef`.

### P2 - deterministic discovery (`fluzo-runtime`)

- `crates/fluzo-runtime/src/context.rs`: the PRD 16 recipe, tracked-only
  and secret-bearing exclusion enforced against `git ls-files`, diff and
  status redaction per file segment, `considered` / `redacted` records,
  `applicable_instruction_files` root-outward, `budget_from_settings`.
- 23 tests against a real temporary Git repository. Commit `a59189b`.
- Bugs caught by tests: NUL-delimited index split; `split_inclusive`
  attaching the `diff --git ` separator to the *previous* segment, which
  put each header at the end of the wrong block and discarded every body.

### P3 - compaction engine (`fluzo-runtime`)

- `crates/fluzo-runtime/src/compaction.rs`: `threshold_reached`, `plan`,
  `segment_budget`, `is_compactable`, `CompactionTransport` (dyn-compatible,
  boxed future, no new dependency), `run`, `commit`, `reverse`,
  `CompactionArtifact` with the audit payload, closed `CompactionBlocked`.
- 25 tests (19 trigger/planning/running, 6 reversibility + durable commit
  against the real SQLite journal). Commit `19ed8a2`.
- Dead branch removed during the work: a pre-flight "preserved items alone
  fill the window" check is unreachable, because if anything is eligible to
  summarize then `assemble` already proved the preserved items fit
  alongside it. The real fit failure is caught afterwards by
  `SummaryTooLarge`; the zero-capacity case keeps its own variant.

### P4 - documentation

- `CONTEXT.md` (contract, trust model, discovery recipe, exclusion rules,
  budget arithmetic, compaction semantics, blocked-reason table, verified
  commands, gaps).
- `AGENTS.md` AGT-02 paragraph added after the AGT-01 gateway paragraph.

### Verification record

- `cargo fmt --all -- --check` - clean.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` - clean.
- `cargo test --workspace --locked --offline` - 881 passed, 0 failed, 10 ignored.
- `cargo build --workspace --locked --offline` - clean.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` - 90 passed, OK.
- `python3 scripts/check_dev_setup.py` - pass (foundation graph, toolchain,
  LSP config, 12 skills).
- Live inference: `not_run`. No provider contacted by any test in this
  increment; the compaction transport is exercised through the scripted
  seam.

### Not done here (tracked as gaps in `CONTEXT.md`)

- Production `CompactionTransport` adapter over `ProviderGateway` (the
  wiring that hands it a live `ChatRequest` / cancel pair) ships with the
  agent loop.
- No caller triggers compaction automatically yet; the engine is invoked
  explicitly.
- TUI/CLI surface for pack composition, omissions and compaction.
- Skill discovery, installation and updates.
