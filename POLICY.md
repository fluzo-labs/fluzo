# Policy, grants and session YOLO

RUN-02 (#16) adds the decision layer for permission-sensitive work:
origin-aware permission resolution, exact-scope grants with one-time
consumption, and a session YOLO mode that removes prompts without
removing policy. PRD v0.2 sections 12, 26.1 and 42.1 and Architecture
A05 are the authority; this file records the implemented surface.

Nothing here executes anything. The layer produces a decision and the
caller acts on it.

## Where things live

| Location | Contents |
| --- | --- |
| `fluzo-core::policy` | Pure rules: origins, resolution, subjects, grants, denials, decisions. No I/O, no clock, no terminal. |
| `fluzo-runtime::authorization` | The live ledger: issued grants, current denials, consumption, expiry, revocation, and the composed decision. |
| `fluzo-runtime::session_auth` | Session YOLO, host approval capability, and the outcome the caller receives. |

## Permission resolution

A permission value reaches the engine from one of four origins:
`BuiltIn`, `Repository`, `ExplicitFile`, `CommandLine`. Only one of
them is restricted.

**The repository may narrow and may never widen.** `.fluzo` is data, not
consent. A repository `deny` against an operator `allow` takes effect.
A repository `allow` against an operator `ask` or `deny` does not: the
operator's value governs and the attempt is recorded in
`ResolvedPermission::ignored_repository_broaden` rather than dropped
silently, so the file's attempted effect stays knowable.

Among operator origins, `CommandLine` outranks `ExplicitFile` outranks
`BuiltIn`. A repository narrowing applies even over a command-line
`allow`; the resolved value carries `origin = Repository` so the
operator can see who narrowed it.

Restriction ordering is `Allow < Ask < Deny`, which makes "narrow only"
a comparison rather than a per-key special case.

## Subjects and what they cover

`PolicySubject` is `Filesystem`, `Shell`, `Network`, `Git`, `Credential`
or `Unknown`. Each carries the full scope the operator was shown.

A shell subject carries the exact executable, the exact argument vector,
the shell mode (`Direct` or `ExplicitShell`), the working directory and
the explicitly supplied environment overrides. Approving one says
nothing about any other. There is no prefix, subset or "close enough"
matching anywhere in grant comparison; that absence is the feature PRD
12 asks for, and a matcher general enough to be useful is a matcher
general enough to be the bug.

`Direct` and `ExplicitShell` are different scopes because a shell
interprets its argument and a direct request does not.

### Subjects with no permission knob

`policy_key()` returns `None` for filesystem, credential and unknown
subjects. Those fail closed to `AskUser`.

Filesystem access deliberately has no `policy.filesystem` key. The
RUN-01 jail decides it, and adding a permission knob here would create a
second and weaker authority over something that already refuses.

An `Unknown` subject never becomes `Allow` by default. A subject this
build does not understand is a subject that must be asked about.

## Grants

A grant is an authorization for one exact operation, bound to a
workspace fingerprint (not a path, so a grant record cannot leak the
workspace location) and to an execution backend.

Matching is exact equality on subject, workspace and backend, and the
grant must be `Issued`.

**One-time consumption is a state transition, not a boolean.**
`consume(grant_id, dispatch_id)` moves `Issued -> Consumed` before the
dispatch happens. A second `consume` returns
`AlreadyConsumed { dispatch_id }` naming the first dispatch, and leaves
the first record untouched. There is no transition back to `Issued`: a
grant that could be un-consumed is a grant that can be replayed.

**Uncertainty keeps the grant spent.** If a dispatch outcome cannot be
confirmed, `mark_outcome_uncertain` records that, and the grant stays
consumed. This mirrors `WriterTicket::uncertain` in RUN-01: "we do not
know whether it happened" is not "it did not happen", and treating it as
the latter would permit a duplicate side effect.

**Revocation affects future dispatches only.** Revoking a consumed grant
returns `NotIssued`; it does not undo completed work.

**Task grants expire and are not restored.** `expire_task` moves every
issued grant for that task to `Expired` on completion, failure,
cancellation or interruption.

The ledger is in-memory on purpose. PRD 12 says task grants must not be
restored automatically on recovery, so a restart starts with none — the
safe reading, and the one a migration cannot get wrong. What is durable
is the audit record of each request, grant, consumption, denial, expiry
and revocation, written through the OBS-01 audit path by the caller.

### File version binding

A filesystem grant may carry the `FileVersion` the operator saw. A
version-bound grant covers only that version: a differing `size`,
`inode`, `device` or `modified_ns` stops it covering, as does a
proposal that carries no version at all. This is what makes "approval
of a pre-existing change" stop meaning what it meant once the file moved
under it.

## Denials

A `Denial` covers either a whole `SubjectKind` or one exact subject.
Denials are checked before grants and before permissions, so no ordering
of grants and no permissive setting can work around one.

## Session YOLO

YOLO is applied to the decision after every denial and permission rule
has run, and it converts `AskUser` only. There is no path in which a
`Deny` becomes an `Allow`.

The resulting reason is `YoloSuppressesApproval`, so the audit trail
attributes the authorization to the mode rather than to a user who
never approved that operation. YOLO issues no grant.

YOLO is process-memory only. There is no settings key for it: a `.fluzo`
containing `yolo = true`, or `policy.yolo = true`, is rejected as an
unknown field. A new session always starts with it off, so resume never
restores it.

YOLO does not bypass the workspace boundary (RUN-01 jail), a stale file
version, a consumed grant, budgets, or cancellation. Those live outside
this layer and are not reachable from it.

## Headless approval

The host declares its capability — `Interactive` or `Headless` — when
the service is built. Headless is a property of the environment known
before the request, not something discovered by a timeout on a prompt.

Nothing in the resolution path reads a terminal, waits on a channel or
holds a deadline for input, so "does not hang" is structural.

A headless run that needs approval returns `ApprovalUnavailable`, which
carries the same `ApprovalRequest` shape the interactive host would have
rendered. It is deliberately distinct from `Denied`: the operation is
not forbidden, it is unexecutable *here*. Conflating them would tell the
operator they refused something they were never shown.

A denied operation produces no request in any mode, and
`can_approve(subject)` lets a host skip prompting for something that can
never be approved.

## Focused checks

```sh
cargo test -p fluzo-core --lib --locked --offline policy
cargo test -p fluzo-runtime --lib --locked --offline authorization
cargo test -p fluzo-runtime --lib --locked --offline session_auth
```

The headless non-blocking contract is proven by a child process that
receives a stdin pipe the parent keeps open and never writes to. The
child must decide and exit without reading it; the parent waits on a
channel with a deadline and fails if the child produces nothing. A
blocking read would hang the child rather than see EOF, which is what
makes the fixture test the real property instead of a generous timeout.

## Not covered here

- No dispatcher calls this yet. There is no task execution loop, so no
  dispatch consumes a grant in production.
- Interactive prompting is not implemented. `ApprovalRequired` is a
  value the TUI would render; the rendering is later UI work.
- Budgets and cancellation (#19) are outside this layer.
- The workspace boundary is RUN-01's jail, not a policy key.
- Cross-process grant sharing does not exist; grants are per-session and
  per-process.
- MCP/ACP tool admission is not wired.
