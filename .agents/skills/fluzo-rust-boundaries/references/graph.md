# Inspect the resolved Cargo graph

Read before running metadata or changing manifests. Treat the consumer's architecture as the policy input, not a property Cargo enforces automatically.

## Specify the policy

Record which packages are workspace-owned, which dependency paths are permitted, whether the rule is direct or transitive, and which exceptions have approval. Distinguish normal, build, and dev edges. A dev dependency may be legal for integration tests but unacceptable in shipped code; a build dependency is not a runtime import but can execute during compilation and still matters to supply-chain and architecture policy.

Identify supported targets, default and non-default features, platform-specific dependencies, and mutually exclusive feature sets. Do not assume all features can coexist. Do not export a bootstrap rule forbidding all external dependencies as a general rule for mature projects.

## Resolve deliberately

After checking local Cargo/toolchain configuration and execution risks, this read-oriented command is appropriate when the consumer has a valid lockfile and available dependencies. Run from the consumer root:

```bash
cargo metadata --format-version 1 --locked --offline
```

It is not permission to create a lockfile or download missing crates. Without a valid lockfile or cache, report incomplete evidence and propose the consumer's approved preparation step. Do not drop `--locked` or `--offline` to make a failed check look successful.

For each approved matrix entry, supply its explicit feature flags and target filter supported by the installed Cargo. Inspect `cargo metadata --help` before choosing options. Use `--no-default-features`, selected `--features`, or `--filter-platform` only when they match the configuration under review. A broad default metadata graph may include target-conditional edges; label those rather than claiming they all execute on the host. `--no-deps` cannot establish a transitive graph.

Read `packages`, `workspace_members`, and `resolve.nodes`. If `resolve` is absent, transitive evidence is incomplete. Use package IDs rather than names alone: renamed dependencies, duplicate versions, paths, and registry sources can differ. Inspect resolved `deps` and `dep_kinds`; declared dependencies alone do not tell you which optional edges are active.

Traverse only the edge categories relevant to the stated rule and retain the full witness path for a violation. Apply cycle protection even if Cargo rejects many cycles itself. Report conditional context alongside each edge. Do not let dev-only paths cause a false production finding; do not accidentally ignore a forbidden build path when the policy includes build-time coupling.

## Validate coverage, not just one result

Compare the existing and proposed graph for each supported matrix entry. Test inactive and active optional dependencies, relevant target conditions, and transitive coupling through a third package. A compile success does not prove that the graph obeys the architecture, and one metadata invocation is not feature-matrix coverage.

If a checker refuses all feature declarations or every external package, establish whether that is a deliberate bootstrap guard. Extend the policy only after the consumer approves the new configuration; do not remove guards broadly as part of adding one dependency.

Keep a legal positive case beside negative cases. A checker that rejects every workspace can appear secure while being useless. Record rule, configuration, expected witness, observed result, and any unresolved metadata/tooling limitation.
