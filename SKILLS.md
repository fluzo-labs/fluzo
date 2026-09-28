# Skills research and adoption plan

## Recommendation

Research date: 2026-09-24. This is a project-fit assessment, not a benchmark of
agent accuracy or an endorsement of an entire upstream collection. Repository
READMEs, skill entry points, selected supporting rules, trees and license metadata
were inspected through GitHub. Upstream example code and installers were not run.
The approved initial adoption is now installed as two narrowly adapted third-party
skills and two original project skills; no external hooks/plugins were installed.
The candidate assessments below record the original upstream review.

Start with **one general Rust reference**, a **focused review checklist**, and
small **Fluzo-specific procedures** derived from the accepted architecture. Add
Ratatui and unsafe-review references when their corresponding work begins. Do not
load several overlapping Rust collections into every task or adopt their default
Cargo settings wholesale.

The project's PRD, accepted architecture, supported toolchain and actual code
remain authoritative. These skills help agents develop Fluzo; they do not add
runtime plugins, MCP/ACP support or automatic skill discovery to Fluzo itself.

## Researched external candidates

### 1. General Rust: leonardomso/rust-skills

- Source: [skill entry point](https://github.com/leonardomso/rust-skills/blob/master/SKILL.md).
- Reviewed revision: `fd2a861ab0406a4ac536a55274d14ea6fd1ca9c9`.
- License: [MIT](https://github.com/leonardomso/rust-skills/blob/master/LICENSE), confirmed through GitHub license metadata.
- Observed coverage: 265 rules across 26 categories, including ownership, typed
  errors, async cancellation, bounded channels, testing, Serde and observability.
  Detailed rules are separate files, suitable for task-specific reading.
- Best fit: the initial general-purpose reference for foundation and ordinary
  Rust implementation. Read relevant rule files rather than the entire collection.
- Caveats: its entry point targets Rust 1.96/edition 2024 and recommends release
  settings including `panic = "abort"`, fat LTO and one codegen unit. Fluzo now pins
  Rust 1.98.0 but has not adopted these build tradeoffs. Native CPU optimization
  advice is not a portable Linux x86_64 release baseline.
- Fluzo overrides: owned application-port DTOs are intentional despite general
  borrowing advice. Do not erase typed protocol errors into generic application
  errors, expose Tokio handles to the TUI, replace confinement with ordinary async
  path I/O, or use channel delivery as durable audit. The cancellation rule's
  suggestion to keep work alive through a spawned task still needs explicit
  ownership, bounded shutdown and outcome reconciliation here.

**Decision:** preferred general reference after review/adaptation, not an
unconditional import of all rules or recommended dependencies.

### 2. Rust review: apollographql/rust-best-practices

- Source: [rust-code-review skill](https://github.com/apollographql/rust-best-practices/blob/main/.codex/skills/rust-code-review/SKILL.md).
- Reviewed revision: `eb485a5ddb68e0ded3d79549e994f63ec0a6f6c0`.
- License: [Apache-2.0](https://github.com/apollographql/rust-best-practices/blob/main/LICENSE), confirmed through GitHub license metadata.
- Observed coverage: short severity-based review checklist for ownership, error
  handling, allocations, testing, public APIs and unsafe boundaries, backed by a
  handbook. Good fit for focused pre-PR review without a large routing framework.
- Caveats: the skill shows `--all-feature`, rather than the documented
  `--all-features` spelling; use verified project commands instead of relying on
  option abbreviation. Fluzo must review its supported feature combinations
  regardless. The entry point also uses `globs` and `alwaysApply`; do not assume
  these agent-specific fields enforce activation in Crush.
- Fluzo overrides: supplement memory/code-quality findings with durable intent,
  permission/version checks, capacity uncertainty, secret/terminal safety and
  inference-free acceptance. The checklist alone is not a security review.

**Decision:** preferred focused review companion; adapt activation and commands
rather than copying its host-specific configuration.

### 3. Async and lifecycle: actionbook/rust-skills

- Sources: [m07-concurrency](https://github.com/actionbook/rust-skills/blob/main/skills/m07-concurrency/SKILL.md)
  and [m12-lifecycle](https://github.com/actionbook/rust-skills/blob/main/skills/m12-lifecycle/SKILL.md).
- Reviewed revision: `5c40d3ad785193231b7d0dbfb8e1eb447e5edd94`.
- Observed strengths: targeted questions about sharing, Send/Sync, workload type,
  resource ownership and cleanup. More useful as selected modules than a universal
  Rust router for this already-designed project.
- Caveats: the collection includes Claude-oriented plugins/hooks/agents and
  cross-skill dependencies. Skills-only support is advertised but integration with
  this Crush session was not tested. Its concurrency tables simplify async
  thread-safety; `Arc<T>` and async code are not unconditionally Send/Sync.
- Critical mismatch: generic automatic resource release through `Drop`, global
  config examples and returning pool permits on scope exit are insufficient for
  Fluzo's repository isolation and uncertain dispatched inference. Local scope
  exit does not prove remote completion.
- License status: README advertises MIT, but GitHub's license endpoint returned
  404 and the inspected recursive tree contained no license/copying/notice file.
  Resolve the licensing evidence before vendoring or redistributing its content.

**Decision:** secondary reference only for now; do not install the whole plugin
collection or treat its lifecycle examples as Fluzo's execution contract.

### 4. TUI: ZhangHanDong/ratatui-skills

- Source: [Ratatui skill](https://github.com/ZhangHanDong/ratatui-skills/blob/main/SKILL.md).
- Reviewed revision: `b413430707a30cf1241c0598b3e4578526a5d009`.
- Observed coverage: terminal setup, layout, widgets and styling, with separate
  references/modules. A strong topical match for UI-01 through UI-04.
- Caveats: it prescribes Ratatui 0.30 and edition 2024. Validate compatibility
  against Fluzo's selected toolchain before using its APIs. Simple blocking event
  loops are teaching examples, not the production input/control architecture.
  Immediate-mode rendering does not justify reparsing an entire transcript on
  every frame. It does not replace Fluzo's PTY, sanitization, focus or latency tests.
- License status: GitHub's license endpoint returned 404 and the inspected tree
  contained no license/copying/notice file. Public availability is not permission
  to vendor the material; clarify licensing before redistribution.

**Decision:** useful reading reference during the visual milestone; prefer an
original Fluzo TUI procedure over vendoring it while licensing is unresolved.

### 5. Unsafe review: google/rust-skills

- Source: [unsafe-rust-review](https://github.com/google/rust-skills/blob/main/unsafe_rust_review/SKILL.md).
- Reviewed revision: `4b4f8b25d19c5ad3ad78b23c4c55ac35adad75a3`.
- License: [Apache-2.0](https://github.com/google/rust-skills/blob/main/LICENSE.md), confirmed through GitHub license metadata.
- Observed focus: explicit proof obligations for unsafe APIs/operations, pointer
  validity, aliasing, initialization, safe callers and dependency assumptions.
  The repository also has a separate experimental variant; it is not needed here.
- Best fit: targeted review if platform adapters actually introduce unsafe code,
  FFI or raw-pointer abstractions. Prefer safe established APIs where sufficient.
- Caveats: the entry point is extensive, so load it only for relevant work. This
  review is about Rust soundness, not proof of filesystem confinement, consent,
  process ownership or resistance to a malicious same-user process. Miri/tests
  are evidence, not a substitute for safety reasoning or an OS security audit.

**Decision:** conditional specialist, not a baseline always-loaded skill. The
assessment inspected its mission and selected review sections, not every reference
or evaluation artifact; complete that audit before installation.

## Fluzo-specific skills worth authoring

The first two original procedures below are installed. The remaining procedures
are deferred to their milestones, not represented by empty skill placeholders.
They cover project-specific risks that generic Rust guidance does not resolve.
Keep each entry point short and link to exact design sections rather than copying
another architecture specification.

| Priority | Proposed skill | Trigger and concrete procedure | Design/issue anchors |
| --- | --- | --- | --- |
| First | `fluzo-rust-boundaries` | Identify crate owner, inspect production dependency edges, check owned DTOs/typed errors and run implemented boundary tests | Architecture sections 3, 5.4; #1, #3, #4 |
| First | `fluzo-deterministic-testing` | Map requirement to regression, isolate repository/config, define strict HTTP steps, assert no non-fixture traffic and independently verify effects | Architecture section 15; #5, #23, #25 |
| Before tools | `fluzo-execution-safety` | Trace authorize/commit/dispatch/result, inject crashes and cancellation, reconcile unknowns, validate grants and writer ownership | A02-A05; #11, #15-#19 |
| Visual milestone | `fluzo-tui-review` | Review reducer/port separation, input priority, bounded rendering, escape decoding, approval focus, snapshot/PTY cases and terminal restoration | A07; #6-#10, #24, #26 |
| Storage milestone | `fluzo-storage-recovery` | Verify PRAGMAs, intent transactions, artifact synchronization, backups, partial purge and active/shared references | A03; #11, #12, #18 |
| Diagnostics milestone | `fluzo-observability` | Verify audit versus sinks, pinned GenAI mapping, single-source usage, three-signal correlation, privacy/cardinality and outage behavior | A09; #13, #14 |

A separate generic test framework or a large set of skills per crate is unnecessary.
Start with boundaries and deterministic testing; introduce the remaining procedures
as corresponding executable behavior and validated commands become available.

## Crush adoption and security checks

The installed Crush configuration skill documents automatic discovery in
`.agents/skills`, `.crush/skills`, `.claude/skills` and `.cursor/skills`. Prefer
project-owned `.agents/skills/<name>/SKILL.md` for any future portable local skills;
no extra skill-path configuration is required for that documented default.
Four installed entry points now use this directory. A currently running Crush
session may need the project reopened to discover newly added skills/configuration;
structural validation does not prove automatic model selection.

Before adopting an external skill:

1. Pin the full reviewed commit, verify the entry point and every required
   reference/script, and record origin, license and local modifications. The links
   above follow upstream branches; use the recorded revisions for reproducibility.
2. Preserve applicable third-party licenses/notices. Missing license evidence is a
   distribution blocker, not something to infer from a marketplace badge.
3. Review scripts, hooks, shell commands and network destinations before execution.
   Do not run marketplace installers, enable broad permissions, or load a cloned
   repository's agent configuration merely to read documentation.
4. Retain the skill's required reference files and relative layout. Copying only
   `SKILL.md` can leave broken dependencies; audit host-specific tools/frontmatter.
5. Narrow triggers. Load selected references on demand; do not let generic Rust,
   TUI and lifecycle routers all redefine the same task workflow.
6. Resolve conflicts with the accepted Fluzo architecture, toolchain and test
   profiles. A skill cannot authorize real inference, publication, destructive
   cleanup, new product scope or weaker security defaults.
7. Validate activation and behavior in Crush on disposable synthetic fixtures.
   Installation success is not evidence that its recommendations compile, preserve
   cancellation/ownership, or improve review quality.

Do not add Cargo tools, libraries or CI jobs solely because a skill lists them.
The bootstrap now fixes Rust 1.98.0, formatter, linter and offline test commands.
Optional property tests, Loom, Miri, snapshot tools or benchmarks still need a
concrete use case and compatibility check.

## Installed adoption and validation

| Installed skill | Source and local changes |
| --- | --- |
| `rust-practices` | Fluzo collection version; historical leonardomso attribution and MIT license retained |
| `rust-review` | Fluzo collection version; historical Apollo attribution and Apache-2.0 license retained |
| `fluzo-rust-boundaries` | Replaced by the pinned fluzo-labs/fluzo-skills version, including graph/protocol references |
| `fluzo-deterministic-testing` | Replaced by the pinned fluzo-labs/fluzo-skills version, including isolation/simulation references |
| `tui-design` | Added from fluzo-labs/fluzo-skills for terminal interaction, safety and verification |

Each adapted skill contains ORIGIN.md with a full upstream revision and its
retained LICENSE. Modified files identify the adaptation. No unlicensed
Actionbook/Ratatui material was copied. Their adoption remains blocked on licensing
review. The original Fluzo tui-design skill is now installed; execution/storage/
observability specialists remain deferred until their milestones. Google unsafe
review is conditional on relevant code (currently unsafe
is forbidden). These are planned later phases, not failed installations.

`python3 scripts/check_dev_setup.py` verifies entry-point metadata, reference links,
licenses, pinned provenance, toolchain/config and the resolved Cargo graph.
`python3 -m unittest discover -s scripts -p 'test_*.py'` checks nineteen positive and
negative cases, including forbidden edges, inactive Cargo features, broken
references, missing licenses, LSP deadlines/EOF and bounded Cargo verification. This is structural/regression evidence, not a measured improvement
in model behavior; model-driven activation/calibration remains unverified.

## Shared Fluzo workflow skills

Installed from [fluzo-labs/common-skills](https://github.com/fluzo-labs/common-skills),
release `v1.2.0`, pinned to `b73053c28ba6e3fc1e4993d47fce933a5d861e85`.
All seven entry points and twenty-one references are installed byte-for-byte;
the update from v1.1.0 was reviewed before replacing the verified local copies.
Each folder additionally contains the upstream MIT LICENSE and a local ORIGIN.md.

| Skill | When to use |
| --- | --- |
| `issue-refine-github` | Assess unclear or oversized issues and propose refinement |
| `plan-create` | Draft an explicit local plan without replacing the GitHub backlog |
| `plan-execute` | Implement an authorized, reviewable phase |
| `delivery-review-github` | Review evidence and prepare issue/PR delivery |
| `convention-document` | Document a confirmed agreement or correction |
| `git-conventional-commit` | Prepare or create an explicitly requested scoped commit |
| `release-prepare-github` | Prepare changelog/release evidence, with publication separately authorized |

The complete copies live alongside the Rust and Fluzo skills in `.agents/skills`.
No npx installer, upstream script, hook, global configuration or extra permission
was used. No update follows a moving branch. Existing Rust skills and LSP settings
were preserved. Crush discovers the default directory; do not add a duplicate
skill-path or another copy under `.crush/skills`.

`.agents/common-skills.toml` records the source, release, reviewed commit and SHA-256 of each
installed file, including local provenance and license copies. The existing CI
checker now verifies all twelve skills and both collections' exact file sets
and hashes. Regression cases reject edited content, missing references/licenses,
extra resources, a changed revision and linked resources. These are integrity and
portability checks, not a signature, sandbox or proof of agent behavior.

For updates, inspect a specific new upstream commit and diff, preserve local
changes, copy complete reviewed folders, update the manifest and revision guard,
and rerun the documented Python checks. Do not regenerate checksums merely to
hide an unexpected edit. The source can be retrieved with authenticated gh; tests
and ordinary skill usage need no download. GitHub-oriented operations still need
gh access, while git-cliff is optional and is not installed here.

Follow AGENTS.md, the pinned design baseline, project English documentation and
higher-priority tool instructions when generic advice differs. Skill selection or
installation is not approval to commit, publish, merge, change Project state or
release. These procedures are development guidance, not new runtime features.

The v1.2.0 update adds bounded sequential autonomous mode to all seven skills,
with identical self-contained `references/autonomous.md` contracts. Guided mode
remains the default. Explicit scope, per-operation authorization and delegated
content generation can cover continuation without repeated prompts. The default
bounds are one phase, twelve skill invocations, two correction rounds and thirty
minutes, with one writer and shared counters across handoffs. Publication stops
at `waiting_review`; merge, releases and permission changes are not autonomous
operations. Installation does not activate that mode or grant any operation.
Project and higher-priority instructions still prevail. Pinned hashes establish
content integrity, not a cryptographic publisher signature or behavioral guarantee.
No permissions, hooks, runtime configuration or Fluzo-collection files changed.

Reopen Crush to refresh changed descriptions and cached instructions. The update
passed `python3 scripts/check_dev_setup.py` and focused skill/tooling tests,
including revision, content, missing-resource and symlink rejection plus identical
autonomous contracts and their entry-point links. The installation also
verifies upstream byte equality and self-contained links; no live backlog
mutations, releases, commit tests or behavioral model evaluations were performed
as installation tests.

## Fluzo design and Rust collection

Installed all five skills from [fluzo-labs/fluzo-skills](https://github.com/fluzo-labs/fluzo-skills),
pinned to `48a1ac36fc229ccbcb1fe671d5dbd9774a567d56`. This supersedes the initial
three-skill installation at `9d17e400d4a98996c59fd0a4129bc579a2d92f41`.
The user explicitly chose Fluzo versions over all same-name installed skills:

- Replaced local `rust-practices` and `rust-review` with the newly published Fluzo adaptations.
- Refreshed `fluzo-deterministic-testing`, `fluzo-rust-boundaries` and `tui-design`;
  their upstream instruction files are unchanged from the previous revision.
- Preserved all seven common-skills folders and their manifest unchanged.

All upstream files are copied byte-for-byte. The two new Rust folders preserve
upstream ORIGIN.md, including historical leonardomso/Apollo revisions; their direct
installation source is now Fluzo, recorded in the collection manifest. MIT applies
except for rust-review, which retains Apache-2.0. The other three folders add only
local ORIGIN.md provenance. No license or original attribution was removed.
`.agents/fluzo-skills.toml` records the source, pinned revision and SHA-256 of every
installed file. The existing integrity checker validates both collections offline.
Regression mutations cover all five Fluzo skills: edited content, missing
references/licenses, extra stale files, revision drift and symlink substitution.

These portable procedures defer to the consumer's actual policy. In this project,
AGENTS.md and the approved PRD remain authoritative: no automatic 429 recovery,
no release of uncertain dispatched capacity, bootstrap feature restrictions and
owned protocol boundaries still apply. TUI examples of editor/interactive-child
handoff do not add that capability to the MVP; minimum layouts and acceptance
remain those of the PRD. No repository-wide upstream crushrc, scripts, global
settings, runtime features or permissions were adopted.

Installation checks verify twelve unique skill names, exact upstream content,
self-contained relative links and preservation of noncolliding skills. They do not
constitute native E2E, visual acceptance or model behavior evaluation. Reopen Crush
to refresh both changed descriptions and newly added skills; do not assume an
already-running session replaces cached skill instructions automatically.

## Rust LSP installation

The selected server is rust-analyzer, not the obsolete RLS. rust-toolchain.toml
pins Rust 1.98.0 with rust-src, rust-analyzer, Clippy and rustfmt. Initial MSRV is
1.98, intentionally matching the tested version; older compatibility is not claimed.
This avoids mixing a distro LSP binary with an unrelated compiler toolchain.

Project crushrc contains only the explicit Rust LSP registration:

```sh
lsp add rust-analyzer --command rustup --args run --args 1.98.0 --args rust-analyzer --filetypes rust --root-markers Cargo.toml
```

No providers, hooks, auto-approvals or global configuration were changed. The server
uses its ordinary cargo-check diagnostics; Clippy remains an explicit check.
The minimal workspace has no build scripts, procedural macros or external
production dependencies. Before adding them, review their execution and any Cargo
configuration: rust-analyzer can execute repository/dependency code and is not a
sandbox. See the official [security documentation](https://rust-analyzer.github.io/book/security.html).

Verification performed locally:

- `rust-analyzer --version`: 1.98.0; required rustup components installed.
- `bash -n crushrc` and `crush dirs --cwd /home/jose/gitrepos/fluzo`: syntax and project-directory discovery checked.
- `python3 scripts/check_lsp.py`: real LSP initialize, definition, cross-crate
  references, rename with passing renamed-workspace tests, and an intentional
  E0308 diagnostic in an isolated offline temporary workspace.
- Five Rust bootstrap tests, seventeen Python regression tests, format, check, Clippy
  and build passed locally. CI is configured; no remote CI result is claimed.

After the user reopened the project, crush_info included the project crushrc and
all four local skills. Source access started a ready Rust LSP client. In Crush,
references returned nine occurrences across core/runtime/TUI, document symbols
resolved RuntimeAvailability and its variant, and diagnostics reported no issues.
The definition tool found one definition but rendered an empty path; reference
and symbol results provide usable locations instead.

Crush reports the active client as `rust_analyzer` while the explicitly configured
`rust-analyzer` entry is still `not_started`. This verifies working Rust LSP access,
not that the explicit launch command was used. No configuration changes were made
solely to reconcile this status-label difference. Cross-crate rename and injected
compiler errors remain verified by the direct fixture, not by Crush's edit tools.
The four skills are discovered and available for on-demand loading; discovery is
not model-driven calibration or proof of improved coding/review behavior.
