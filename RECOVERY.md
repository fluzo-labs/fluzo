# Session recovery and the private task-control channel

RUN-04 (#18) adds the recovery surface: reopening a recorded session
without touching anything, resuming one only after its recorded world
has been revalidated, and a private channel through which another
process can inspect, cancel or force-stop owned work. PRD v0.2 sections
17, 17.1, 17.2, 22.1 and 24 and Architecture A02 and A03 are the
authority; this file records the implemented surface.

Nothing here runs an agent. Recovery reads, reconciles and refuses. The
agent loop that would continue a resumed session is later AGT work.

## Where things live

| Location | Contents |
| --- | --- |
| `fluzo-runtime::store` | `Store::open_read_only`, the SQLITE_OPEN_READONLY connection that cannot create, migrate or write. |
| `fluzo-runtime::recovery` | `RecoveryReader`, the recorded record types, `effective_state()`, deterministic `Replay` with visible gaps. |
| `fluzo-runtime::resume` | `reconcile()` (read-only) and `apply()` (the only writer), budget carry-forward, stale-claim detection. |
| `fluzo-runtime::control` | `ControlServer`, `ControlClient`, the closed `ControlRequest` set, the framed Unix-socket protocol. |

## Read-only open

`Store::open_read_only` opens the existing journal with
`SQLITE_OPEN_READONLY`. The read-only guarantee is enforced by the
driver, not by convention: a write through any method fails with
`SQLITE_READONLY`, which maps to `StoreError::ReadOnly` rather than
collapsing into a generic I/O fault. A read-only open never creates a
store and never migrates one; a store below the target schema version is
refused as `Inaccessible` instead of being silently upgraded by
inspection.

`RecoveryReader` builds sessions from the recorded state and event
rows. Reading a record is not evidence of liveness, so
`effective_state()` maps what a crash leaves behind into what a reader
may claim:

| Recorded | Read as |
| --- | --- |
| `Running` with no `ended_at_ms` | `Interrupted` |
| `Dispatched` with `result_known == false` | `Unknown` |

Replay walks a session's per-session ordinals. A hole in the ordinals
surfaces as `ReplayGap::MissingRecords`, a missing referenced attempt or
operation as `ReplayGap::MissingAttempt` / `MissingOperation`, and an
undecodable payload as `ReplayGap::Undecodable`. Gaps are reported,
never skipped, and never reconstructed from what seems likely.

Deterministic replay means consistent reconstruction from the same
recorded events. It is not deterministic re-execution: opening a
session contacts no model or decision provider, executes no tool,
retries no request, restores no file and dispatches nothing queued.
Selecting a session is not consent to continue it.

## Resume

Reopening and resuming are different acts, so they are different calls.
`reconcile()` computes the whole result through the read-only reader;
`apply()` is the only writer and commits one atomic journal unit.
Inspection that follows a resume request therefore stays non-mutating
until the reconciliation is actually applied, and a fixture proves it
by counting rows across the call.

`reconcile()` revalidates the workspace fingerprint and the live Git
revision before it will produce anything. A fingerprint mismatch blocks
the resume outright. A missing live revision blocks it too: an unknown
revision cannot validate recorded claims, and guessing would be worse
than refusing.

On apply:

- attempts recorded as running become `Interrupted` with an end time;
- operations that may have acted with no confirmed result become
  `Unknown`, and no second operation is created for the same subject;
- a new attempt is created in `Pending`, linked to the attempt it
  follows by a deterministic successor id;
- grants held by operations that never reached a terminal state are
  expired and never reissued.

Budget carries forward rather than resetting silently. The resumed
attempt starts from the consumed total across prior attempts. A prior
attempt whose contribution was not confirmed marks the whole
carry-forward `uncertain`, so the figure cannot be read as a
trustworthy remaining-budget basis. Extending a budget stays a separate
explicit act performed elsewhere.

Pending approvals from an interrupted attempt do not carry over. A
resumed process gets a fresh `AuthorizationService` through
`resumed_authorization()`, which is the only construction path this
layer offers. YOLO is off because RUN-02 keeps it in process memory
with no settings key, so there is nothing to restore and no way for
recovery to turn it on.

A Git revision that has moved past the recorded one does not block the
resume but invalidates the recorded claims. Each is surfaced as a typed
`StaleClaim::VerificationRevision` or `StaleClaim::EditApproval` with
both revisions, and journaled. Re-applying converges on
`ResumeError::AlreadyApplied` instead of duplicating the attempt.

## Control channel

`ControlServer::bind` takes the `WriterOwnership` by value. The channel
cannot exist without the exclusive writer lock and cannot outlive the
ownership it verifies against. The socket is
`.fluzodrive/control.sock` at mode 0600; a symlink at that path is
refused, as is a path that would exceed the Unix socket length limit,
which is checked rather than truncated.

Every request carries the acquisition nonce from the `OwnerRecord` and
the server re-checks `verify_still_owned()` before acting. A token
copied from a dead owner is refused as `stale_owner`. The pid is never
used as proof of anything.

`ControlRequest` is a closed set of four variants:

| Variant | Effect |
| --- | --- |
| `inspect_session` | Read-only. |
| `task_status` | Read-only. |
| `cancel` | Enqueues a cooperative cancellation for the owner runtime. |
| `force_stop` | Requires `confirmed: true`; otherwise returns `confirmation_required` and signals nothing. |

There is deliberately no variant that enables YOLO, acquires writer
ownership, resumes a task or dispatches work. Adding one is a reviewed
change to the enum, not something a client reaches by sending a name:
an unknown wire kind is refused as `unknown_request` rather than mapped
to the nearest known action, so a typo cannot become a mutating call.

Frames are a 4-byte big-endian length and a JSON body, capped at 64 KiB.
The declared length is checked before any allocation, so a hostile
header cannot make the reader grow to the sender's choosing. The server
greets with the protocol version before reading, so a client that
reaches a foreign or stale socket learns that before it sends a request
it would otherwise retry.

The channel carries identifiers and booleans only. No credential,
endpoint or session payload crosses it, and a refusal returns a stable
code with an empty detail rather than echoing what the caller sent.

## Limits

The socket is reachable by the same user. It keeps other *processes*
out, not the user's own shell, and is not a security boundary against a
cooperating local account. This matches the RUN-01 confinement boundary:
native file confinement is race-aware containment, not an OS sandbox.

Cross-instance inference coordination, provider rate-limit recovery, a
daemon or service that survives the owner process, a TUI session browser
and forensic erasure are all out of scope here.

## Verification

```sh
cargo test -p fluzo-runtime --lib --locked --offline recovery
cargo test -p fluzo-runtime --lib --locked --offline resume
cargo test -p fluzo-runtime --lib --locked --offline control
cargo test --workspace --locked --offline
```

The control fixtures use real Unix sockets and real process locks. The
cross-process cases spawn the child as a role, `control_child_role`,
which is `#[ignore]`d and only runs when spawned by its parent; running
it alone fails its role assertion. Synchronisation is by stdout
announcement, not sleep. The child proves it was served and is then
refused `HeldBy` when it tries to become the writer, while the parent
proves it still holds the lock.

`cargo test -p fluzo-runtime --lib --locked --offline control_child_role`
alone proves nothing.
