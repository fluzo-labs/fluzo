---
plan: run03-native-tools-and-owned-process-lifecycle
issue: fluzo-labs/fluzo#17
seed: RUN-03
milestone: M4 Safe Execution
status: approved-for-execution
created: 2026-10-15
baseline:
  prd: 60c5b0732fb710cdf705476cee8d9156a5ecd971 sections 13, 17.2, 21.3
  architecture: 60c5b0732fb710cdf705476cee8d9156a5ecd971 decision A05
---

# RUN-03: native tools and owned process lifecycle

## Source and revision

- Issue: <https://github.com/fluzo-labs/fluzo/issues/17> (RUN-03, P0, M4).
- PRD v0.2 sections 13 (tool system), 17.2 (graceful cancel and force
  stop), 21.3 (child-process input and injected environment).
- Architecture v0.1 decision A05: root-relative native file tools; direct
  executable/argument requests by default; explicitly selected shell;
  closed stdin; reviewed environments; owned process groups. Fail closed
  on unprovable containment; arbitrary programs remain unsandboxed.
- Prior merged work this branch builds on: RUN-01 confinement and writer
  ownership (#99), RUN-02 policy/grants/YOLO (#104), DAT-02 chain,
  OBS-01 diagnostics.

## Goal

Give the runtime a way to run a program it owns: an explicit argv request
with no shell interpolation, closed stdin, an environment built rather
than inherited, bounded output capture, and cancellation that reaches the
owned process group and nothing else. On top of that, a read-only Git
inspection path that cannot be turned into an unapproved program launch
by repository configuration, and a tool dispatch seam that routes every
request through the RUN-02 policy layer and the RUN-01 jail.

## Scope and exclusions

In scope:

- Process request types: direct argv by default, explicit shell as a
  separate explicitly-selected mode.
- Environment construction from a documented baseline plus authorized
  injections. No wholesale inheritance.
- Closed stdin, no controlling terminal, captured stdout/stderr with
  bounded buffers that keep draining after truncation.
- Process-group ownership established at spawn and verified before any
  signal is sent.
- Graceful termination with a bounded grace period, escalation to forced
  termination, and an explicit uncertain outcome when neither is
  confirmed.
- Read-only Git inspection with external diff/textconv/pager/monitor
  hooks and credential prompting disabled, and a closed set of
  inspectable subcommands.
- Tool dispatch that maps a request to a RUN-02 policy subject, honours
  denials, consumes one grant per dispatch before the side effect, and
  routes file access through the confinement jail.

Excluded, deliberately:

- The agent loop and any model-driven tool selection (AGT chain).
- Budgets and capacity admission (#19).
- Force-stop as a user-facing task control with `--yes` and confirmation
  dialogs (PRD 17.2). This increment builds the mechanism the control
  calls; the control surface itself is later work.
- Interactive PTYs. PRD 21.3 makes them unsupported in the MVP.
- An OS sandbox. A05 explicitly disclaims it.
- A public plugin loader and MCP/ACP tool wiring.
- Network tools and provider calls.

## Verified context

- `fluzo-runtime/src/confinement.rs` provides `WorkspaceJail` with
  root-relative `openat`-style handles, `O_NOFOLLOW` on every component,
  protected-name refusal, symlink rejection in any position,
  non-regular-inode rejection and hard-link (`SharedLink`) refusal.
- `fluzo-runtime/src/workspace.rs` provides `WriterOwnership` with
  `flock`-backed cross-process ownership, `WriterTicket` with
  `completed`/`uncertain`, and release-at-quiescence.
- `fluzo-core/src/policy.rs` provides `PolicySubject` (`Filesystem`,
  `Shell`, `Network`, `Git`, `Credential`, `Unknown`), `ShellScope`
  with `ShellMode::{Direct, ExplicitShell}`, `ExecutionBackend::{
  NativeLocal, ExplicitShell}`, `Grant`, `Denial`, `PolicyDecision`.
- `fluzo-runtime/src/authorization.rs` provides `AuthorizationLedger`
  with exact-equality `grant_for`, `consume` before dispatch,
  `mark_outcome_uncertain`, `decide`.
- `fluzo-runtime/src/session_auth.rs` provides `AuthorizationService`
  with `ApprovalMode::{Interactive, Headless}` and `AuthOutcome`.
- `rustix = 0.38.44` is already locked transitively via crossterm with
  features `alloc, fs, libc-extra-traits, std, stdio, termios`.
- `std::os::unix::process::CommandExt::process_group` is available on
  the pinned toolchain and gives group-leader status without a new crate.

## Decisions, dependencies, and risks

**Process-group ownership is established at spawn and verified before any
signal.** `process_group(0)` makes the child its own group leader. Before
the runtime ever signals, it checks `getpgid(child) == child_pid`. If
that check fails the runtime refuses to signal at all and reports the
process as unowned. Without this check a silently failed group assignment
would mean `killpg` targets Fluzo's own group, which is the exact failure
PRD 17.2 forbids. This is the load-bearing safety property of the
increment.

**Signals go to a verified group id, never to a bare pid and never to a
stored group id.** The group id used for signalling is the one read back
from the spawned child, not one supplied by a caller. A caller cannot ask
the runtime to kill a group it did not create.

**The environment is built, not filtered.** Starting from the parent
environment and removing secrets is a denylist, and a denylist misses the
variable you did not think of. The baseline is an explicit small set
(`PATH`, `HOME`, `LANG`/`LC_*`, `TMPDIR`, `USER`, `TZ`) plus authorized
injections. `env_clear()` runs first so nothing slips through by default.
`BASH_ENV`, `ENV`, `PROMPT_COMMAND` and loader/startup overrides are not
in the baseline and cannot be injected through the tool path.

**Executable resolution is explicit and rejects a relative-path search
expansion.** A request naming `./tool` or a bare name that resolves
through an implicit current-directory search is refused. The approved
executable is resolved against the effective `PATH` before spawn, and
the resolved absolute path is what the approval covers.

**Bounded capture keeps draining after truncation.** Stopping reads at
the cap deadlocks the child on a full pipe. After the cap is reached the
reader keeps draining and discarding, and records `truncated = true`.
This is the difference between a bounded buffer and a stall.

**Cancellation is graceful-then-forced with an explicit unknown.**
`SIGTERM` to the owned group, bounded grace period, then `SIGKILL` to
the owned group. If the group still has members after the escalation and
bounded wait, the outcome is `Uncertain`, not "cancelled". Per the
architecture, unconfirmed termination keeps workspace ownership from
being released optimistically.

**Git inspection is a closed set of read-only operations, not a git
command runner.** The type enumerates what can be inspected. Anything
outside it is a shell request through the ordinary policy path, not a
silent read-only inspection. Repository-supplied helper configuration is
disabled by explicit flags and environment, so a repo setting cannot
turn context inspection into a program launch.

**Dependency graph change: `rustix` gains the `process` feature.**
Required for `killpg`, `getpgid` and signal constants. Verified before
planning: `cargo metadata --locked` shows the resolved set becomes
`alloc, fs, libc-extra-traits, process, std, stdio, termios` on rustix
and adds `prctl` on the already-locked `linux-raw-sys 0.4.15`.
`Cargo.lock` is unchanged and no new package enters the workspace. This
is the same class of change as DAT-02b adding `fs`, and it requires the
reviewed ceilings in `scripts/check_dev_setup.py` to be widened
accordingly. The change is called out in the PR for explicit review.

Risks:

- A child that installs its own signal handlers can ignore `SIGTERM`.
  Covered by the escalation and the explicit uncertain outcome.
- A descendant that `setsid`s away from the owned group escapes group
  signalling. It is reported as escaped rather than silently assumed
  terminated.
- `getpgid` between spawn and signal can race if the child already
  exited and its pid was recycled. The check runs against the pid we
  spawned and a failed lookup is treated as "already gone", never as
  licence to signal a group.

## Phases

### P1: Owned process lifecycle

- Outcome: a request carrying an explicit argv vector, working directory
  and environment spec spawns a program in its own verified process
  group, with closed stdin, no inherited controlling terminal, bounded
  captured output, and cancellation that reaches only that group.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: FND-02 (#2), RUN-01 (#15) for the jail used by callers.
- Affected contracts: new `fluzo_runtime::process` with `ProcessRequest`,
  `EnvironmentSpec`, `OwnedProcess`, `Capture`, `CaptureLimit`,
  `TerminationMode`, `SpawnError`, `CancelError`. No existing signature
  changes.
- Acceptance criteria:
  - A direct request delivers the exact argv to the child with no shell
    interpretation: metacharacters in an argument arrive verbatim.
  - An explicit shell request is a distinct mode whose script is part of
    the request, and a direct request never becomes a shell request.
  - A failed direct spawn does not fall back to `sh -c` or any other
    interpreter; it returns a typed error.
  - The child's stdin is closed: a read returns EOF immediately.
  - The child has no controlling terminal inherited from the runtime.
  - The child environment equals the documented baseline plus authorized
    injections. A sentinel credential present in the runtime's own
    environment is absent from the child.
  - `BASH_ENV` and equivalent startup-injection variables are not
    inherited and are refused as injections.
  - A relative or implicit-current-directory executable is refused.
  - Capture stops growing at the configured byte limit, marks
    `truncated`, and the child still terminates normally because draining
    continues.
  - Cancellation sends `SIGTERM` to the owned group, escalates to
    `SIGKILL` after the grace period, and records which mode terminated
    it.
  - A child that ignores `SIGTERM` is forced, and if it still resists
    the outcome is `Uncertain` rather than reported as terminated.
  - A descendant in the owned group is reached by group cancellation.
  - A descendant that leaves the group is reported as escaped and is not
    signalled.
  - Cancellation refuses to signal any group other than the verified
    owned one, and refuses to signal group 0 or the runtime's own group.
  - If group ownership cannot be verified after spawn, the runtime
    refuses to signal and reports the process as unowned.
- Actions:
  - [ ] Implement the process module with group verification.
  - [ ] Deterministic fixtures: cooperative child, stubborn child,
        descendant, escaping descendant, sentinel credential, oversized
        output, unrelated-group refusal.
  - [ ] Widen the reviewed rustix/linux-raw-sys feature ceilings with a
        recorded justification.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  process`.
- Risks and recovery: additive module; no existing caller changes.

### P2: Safe Git inspection

- Outcome: read-only Git inspection returns repository facts with
  external diff, text conversion, pager, filesystem-monitor hooks and
  credential prompting disabled, and cannot be widened by repository
  configuration into a program launch.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: P1 for the owned-process path.
- Affected contracts: new `fluzo_runtime::git_inspect` with
  `Inspection` (closed set), `GitInspectionError`, `run_inspection`.
  No existing signature changes.
- Acceptance criteria:
  - The inspectable set is closed at the type level; a mutating or
    unlisted Git operation is not representable as an inspection.
  - Inspection runs with pager, external diff, textconv, fsmonitor and
    terminal credential prompting disabled by explicit flags and
    environment.
  - A repository-configured `core.pager`, `core.fsmonitor`,
    `diff.external` or `core.hooksPath` helper is not executed during
    inspection; a sentinel helper proves it by never being invoked.
  - System and user Git configuration are not read.
  - Inspection is read-only: the working tree, index and refs are
    unchanged after each inspection operation.
  - Inspection output is bounded by the P1 capture limit and reports
    truncation.
  - Inspection runs in the owned process group with closed stdin.
- Actions:
  - [ ] Implement the inspection surface on the P1 path.
  - [ ] Fixture repository with sentinel helpers that must not run.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  git_inspect`.
- Risks and recovery: additive; depends on P1 correctness.

### P3: Tool dispatch through the policy layer and jail

- Outcome: a tool request maps to a RUN-02 policy subject, denials win,
  one grant is consumed before the side effect, file access goes through
  the RUN-01 jail, and the outcome carries audit-shaped provenance.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: P1 for process execution, RUN-02 (#16) for the ledger
  and approval service, RUN-01 (#15) for the jail.
- Affected contracts: new `fluzo_runtime::tools` with `ToolRequest`,
  `ToolOutcome`, `ToolDispatcher`, `ToolError`. No existing signature
  changes.
- Acceptance criteria:
  - A denied request never reaches spawn, and the denial reason is
    returned rather than a prompt.
  - A request with no applicable grant in a headless host returns the
    RUN-02 unavailable outcome without blocking.
  - Exactly one grant is consumed per dispatched request; a second
    dispatch of the same grant is refused.
  - A dispatched request whose outcome is unconfirmed marks the grant
    uncertain rather than re-arming it.
  - A file tool cannot reach a path the jail refuses; the jail error is
    returned and does not turn into a shell attempt.
  - A rejected native operation does not automatically retry as a shell
    command.
  - The request's policy subject and the actual argv/env that would run
    are the same values the approval covered; a mismatch is refused.
  - YOLO suppresses a prompt but does not suppress a denial, a jail
    refusal or group-ownership refusal.
- Actions:
  - [ ] Implement the dispatch seam over the existing ledger and jail.
  - [ ] Tests for denial-wins, one-grant-per-dispatch, no
        jail-bypass, no shell fallback after native refusal.
  - [ ] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  tools`.
- Risks and recovery: additive; the seam has no production caller yet.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | done (process.rs, 880 lines) | 29 passed / 0 failed | pending | `cargo test -p fluzo-runtime --lib --offline process` |
| P2 | pending | not_run | pending | none yet |
| P3 | pending | not_run | pending | none yet |

## Next step

Execute P1, then P2, then P3, in one reviewable PR referencing #17 and
the three children. Human review remains the gate; `implemented` is not
`accepted`.

## Evidence and tracking

- Parent issue: <https://github.com/fluzo-labs/fluzo/issues/17>.
- Children: created during the split step of this task; URLs recorded in
  the PR.
- GitHub issues remain the state authority; this document is a design
  snapshot, not a parallel board.
