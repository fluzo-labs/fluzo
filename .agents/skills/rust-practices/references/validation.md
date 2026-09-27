# Validation cases

Use these cases for an authorized consumer review or a disposable fixture. Run only commands supported by that consumer; do not install a generic Rust test stack to follow the skill.

| Case | Expected decision or evidence |
| --- | --- |
| Read-only local function clones a large collection without need | Inspect callers and propose a borrow if lifetimes and behavior permit it |
| Versioned owned snapshot isolates consumers from later mutation | Preserve ownership; do not reject the clone solely for allocating |
| Clone of a shared handle | Assess actual ownership and cost, not an assumed deep allocation |
| Invalid user configuration | Typed error with safe context, not panic or silent defaults |
| Uncertain remote side effect | No blind retry or claim of cancellation; follow reconciliation policy |
| Bounded queue with unlimited producers or message sizes | Report missing work/byte bounds and their impact |
| Full queue blocks cancellation | Keep control reachable under the consumer's documented mechanism |
| Optional diagnostics drop under pressure | Account for bounded loss without treating mandatory audit as optional |
| Claimed optimization without measurements | Label it a hypothesis; do not fabricate performance evidence |
| Missing toolchain, cache, or LSP | Report the limitation; no automatic installation or permission expansion |
| Proposal-only request | No implementation, commit, push, or publication |

Verify the intended success and failure behavior before broad checks. Record commands, cwd, exact content/revision, environment, and actual outcomes. Static review of these instructions is not an executed Rust regression or evaluation of agent behavior.

For distribution, copy the complete folder into an isolated temporary directory, confirm all operational links remain inside it, and preserve LICENSE and ORIGIN.md. External URLs in provenance and legal text are attribution, not executable dependencies. Do not replace the historical revision or license hash just to conceal an unexpected change.
