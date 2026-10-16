# Context engine (AGT-02)

Scoped context, explicit skills and automatic compaction. Implements PRD v0.2
§16 (Context Engine), §16.1 (Context budget and automatic compaction) and
§20 (Skills); Architecture §11 (Context and Compaction) and decision A06,
at design revision `60c5b0732fb710cdf705476cee8d9156a5ecd971`.

The controlling idea: context is assembled from a fixed recipe under a typed
budget, and nothing about what the model sees is decided by a relevance
heuristic. Every item carries where it came from, how much it costs and how
much trust it earns.

## Modules

| Module | Crate | Role |
| --- | --- | --- |
| `context.rs` | `fluzo-core` | Pure ledger: trust tiers, source refs, token estimates, budgets, items, packs, deterministic assembly, explicit skill selection. No I/O, no clock. |
| `context.rs` | `fluzo-runtime` | Discovery: reads the confined workspace and produces the initial pack. |
| `compaction.rs` | `fluzo-runtime` | Compaction: trigger, planning, bounded provider calls, artifact, durable commit, reversal. |

The split is the same one used by admission: the rules that gate real
behaviour live in `fluzo-core` as pure data so they can be asserted
directly, and the live wiring sits in `fluzo-runtime`.

## Trust tiers

Five tiers, ordered by how much authority they carry. Exactly one may grant
anything.

| Tier | May authorize | Examples |
| --- | --- | --- |
| `RuntimeAuthority` | yes | budgets, approvals, YOLO, observed workspace revision |
| `UserIntent` | no | the user's request for this task |
| `Instruction` | no | `AGENTS.md`, explicitly selected skills |
| `RepositoryContent` | no | file contents, `git status`, diffs |
| `ModelText` | no | assistant turns, compaction summaries |

`TrustTier::can_authorize()` returns true for `RuntimeAuthority` only. A
skill's own prose, a repository file and a compaction summary are all
non-authoritative by type, not by convention. `ContextPack::
authoritative_sources()` makes the claim checkable: a discovered pack
contains exactly one, the observed workspace revision.

## Estimates are not usage

`TokenEstimate` is a distinct type from provider-reported `GenAiUsage` and
records its own method (`CharactersOverFour`, `SumOfItems`). A pack total is
a derived estimate and can never be read as a fresh provider measurement.
Provider-reported usage stays in `CompactionArtifact::provider_usage`,
where `None` means unknown and is never replaced by zeros.

## Deterministic assembly

`assemble(budget, items)` sorts by a fixed total order the builder cannot
influence: required before optional, then tier rank, then source identity.

- Required content that does not fit blocks with
  `ContextError::RequiredDoesNotFit`. It is never dropped.
- Optional overflow is recorded in `pack.omitted` with its provenance and
  an `OmissionReason`. A caller can always answer what the model did not
  see.

## Discovery recipe (PRD §16)

Fixed order, no scoring:

1. The user's request — `UserIntent`, required.
2. The observed workspace revision — `RuntimeAuthority`, required. This is
   the value a later check re-verifies, never one a summary may restate.
3. Applicable nested `AGENTS.md` files, root-outward — `Instruction`,
   required. A target of `crates/core/src/lib.rs` considers the root file,
   then `crates/AGENTS.md`, then `crates/core/AGENTS.md`, then
   `crates/core/src/AGENTS.md`.
4. `README.md` — `RepositoryContent`, optional.
5. Git evidence: `status`, staged diff, worktree diff — optional.
6. Explicitly selected skills — `Instruction`, required.

The whole repository is never sent. Anything beyond this recipe arrives
later through the policy-gated read and search tools, which is a different
authority path.

### Exclusion rules

Both apply to every file discovery reads automatically, and both are
recorded per file in `DiscoveredContext::considered`:

- **Tracked only.** A candidate must appear in `git ls-files`. Ignored and
  never-added files are out by construction rather than by a pattern list
  that can drift. The listing is NUL-delimited and split accordingly.
- **Secret-bearing names excluded**, even when tracked. A closed list of
  names, prefixes and extensions (`SECRET_FILE_NAMES`,
  `SECRET_NAME_PREFIXES`, `SECRET_FILE_SUFFIXES`).

The second rule matters most for Git evidence: a tracked `.env` really is
tracked, and a modification puts its value in the diff. Diff and status
output are therefore filtered by the same name rules before becoming
context, and the removed paths are listed in `DiscoveredContext::redacted`.
Diffs are dropped per file segment rather than per line, so a redaction
cannot leave a header whose body was removed.

Neither rule is a security boundary. Explicit reads still require the
applicable credential and filesystem policy; these only keep automatic
assembly from volunteering sensitive material.

## Skills are explicit only

`ExplicitSkillSelection` is constructible solely from a list of ids.
Nothing in the module maps a task, file or diff to a skill, so implicit
activation is structurally unavailable rather than merely discouraged.

- An empty selection loads nothing.
- `resolve_explicit` refuses unknown ids (`SkillError::Unknown`) and
  duplicates (`SkillError::Duplicate`) rather than quietly tolerating them.
- Resolved skills enter the pack as `Instruction` tier with their source
  link, and never authorize.

## Budget

`ContextBudget` = model window − output reserve − protocol overhead −
safety reserve, validated before anything is read. A window that the
reserves swallow is refused (`BudgetError::ReservesLeaveNoRoom`); a zero
window is refused rather than treated as unlimited.

Derived from the typed settings (A06): `context_window`,
`compaction_threshold`, `compaction_max_output_tokens`,
`tool_context_max_bytes`, `safety_reserve_tokens`, `safety_reserve_fraction`.
The safety reserve is the configured absolute floor plus the configured
fraction of the window — a floor alone is meaningless on a large window and
a fraction alone leaves none on a small one.

Per-item content is cut at `tool_context_max_bytes`. Truncation is recorded
on the item (`truncated`), never implied by absence, and the cut is
re-estimated and never splits a multibyte character.

## Automatic compaction (PRD §16.1)

Four properties, each structural:

- **Bounded.** The plan fixes the segment count before the first provider
  call. More than `MAX_COMPACTION_SEGMENTS` (8) is refused, so a context
  that keeps growing cannot keep the engine looping.
- **Observable.** Every applied run ends in a durable journal event
  (`compaction_applied`) naming what the summary covers, the tokens it
  replaced, its own estimated cost, the workspace revision it describes
  and the provider-reported usage. The journal record alone is enough to
  audit the decision.
- **Reversible.** The summary is an *additional* item. The items it stands
  in for are never deleted, and `reverse()` reconstructs the pre-compaction
  context from the artifact's recorded coverage.
- **Non-promoting.** The summary is `ModelText` with `Optional` retention.
  Required items and any `RuntimeRecord` are never compactable
  (`is_compactable`), so a lossy restatement cannot stand in for an
  instruction or a runtime grant.

Trigger: `compaction_threshold` × effective input. The provider seam is a
dyn-compatible trait returning a boxed future — no new dependency — and
calls go out as `RequestClass::Compaction` through the AGT-01 gateway:
foreground work that pays admission, not an exemption. Cancellation is
checked *before* each call, never during, so the number of calls that
actually left the process is always known from the outcome.

### Blocked reasons

Closed set, no `Other` and no silent give-up:

| Reason | Meaning |
| --- | --- |
| `NotNeeded` | below the threshold |
| `NothingToCompact` | at the threshold, nothing eligible |
| `NoSegmentCapacity` | reserves leave no room for a call at all |
| `SegmentOverCapacity` | one item exceeds one call; refused rather than silently truncated |
| `SegmentLimitExceeded` | needs more calls than the ceiling |
| `NoEndpoint` | no compaction endpoint configured |
| `ProviderFailed` | the call failed |
| `EmptySummary` | a segment returned no text; refused rather than erasing it |
| `SummaryTooLarge` | the summary does not fit the space it was meant to free |
| `Cancelled` | cancellation seen before the next call |
| `Store` | the durable commit failed |

Commit goes through `Store::commit_unit` — one `BEGIN IMMEDIATE`
transaction for the state row and the journal event. Either both land or
neither does; a commit against a read-only journal is verified to leave
nothing behind.

## Verified commands

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python3 -m unittest discover -s tests -q
python3 scripts/check_dev_setup.py
```

Test counts: 28 core ledger, 23 discovery, 25 compaction (19 trigger /
planning / running, 6 reversibility and durable commit).

## Gaps

Not covered by this increment:

- The production `CompactionTransport` adapter over `ProviderGateway` is
  defined by the trait but the wiring that hands it a live
  `ChatRequest`/cancel pair ships with the agent loop, not here.
- No automatic compaction is triggered by any caller yet; the engine is
  invoked explicitly.
- Cross-session context sharing, context pinning by the user, and a TUI
  view of pack composition and omissions are separate work.
- Skill discovery, installation and updates are out of scope: only explicit
  selection by id is implemented.
