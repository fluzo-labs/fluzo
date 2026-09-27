# Validation and the Fluzo profile

## Portable calibration cases

Create a temporary, inspected Cargo workspace without external dependencies or remotes. Use neutral package names and an explicit test policy. Run with the available reviewed toolchain and offline dependencies. Do not change the consumer workspace to insert forbidden edges.

| Case | Expected result |
| --- | --- |
| Approved entry -> service -> domain path | Accepted with the full resolved path available |
| Presentation -> service when forbidden | Rejected with a direct path witness |
| Presentation -> bridge -> service | Rejected with a transitive witness, even if the direct import looks neutral |
| Forbidden optional edge disabled | No active violation for that configuration; declared risk still reported |
| Same optional edge enabled | Rejected under that explicit feature selection |
| Legal dev-only test edge | Not misclassified as a normal production dependency |
| Build-time coupling | Reviewed and classified under the build-edge policy, not silently discarded |
| Approved target-specific edge | Findings identify the filtered target; unsupported targets remain untested |
| Pure package using standard-library I/O | Graph may pass; semantic review still reports the policy violation |
| Owned application snapshot | Not rejected solely because it allocates or clones |
| Missing lockfile or cache | Blocked evidence, no automatic fetch or lockfile modification |
| Review-only request | Findings without implementation or policy writes |

Record which cases were actually executed. Passing metadata fixtures proves limited graph mechanics, not agent judgment, soundness, or exhaustive supported-platform coverage.

## Fluzo profile

Only when the consumer confirms the original Fluzo modular-monolith policy:

| Component | Responsibility | Approved production dependencies |
| --- | --- | --- |
| core | Protocol values and pure policy | No other Fluzo crate; external I/O and terminal/database coupling prohibited |
| runtime | Coordination and adapters | core |
| tui | View state and interaction | core |
| cli | Startup and dependency wiring | core, runtime, tui |

Reject runtime-to-tui, tui-to-runtime, core-to-cli, and indirect paths that recreate those couplings. Keep legal cli-to-runtime-to-core paths. Application ports carry owned/versioned values where specified; concrete provider, DB, transport, or cancellation ownership stays outside the presentation API. Acknowledgement is not completion.

The original bootstrap forbids external production dependencies and feature declarations until deliberate review. This is a temporary consumer gate, not a generic Rust best practice. Later approved dependencies require a refined policy and explicit configuration coverage, not a permanently empty dependency graph.

The original consumer has the following checks, run from its project root only after verifying they exist and inspecting their current behavior:

```bash
python3 scripts/check_dev_setup.py
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo test --workspace --locked --offline
```

Other consumers supply their own commands and policies. Do not import these scripts, hardcode this crate layout, or add missing tools just to follow the skill.

## Distribution checks

Copy this entire folder including its original MIT license. Verify frontmatter, local links, and absence of dependencies on another skill or repository root. Preserve source notices in adaptations. Evidence reports must distinguish the reviewed source revision from later changes and mark unexecuted semantic or agent-behavior scenarios explicitly.
