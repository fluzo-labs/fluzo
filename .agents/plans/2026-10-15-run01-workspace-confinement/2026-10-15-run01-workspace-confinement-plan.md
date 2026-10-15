---
name: "2026-10-15-run01-workspace-confinement"
description: "Workspace identity, cross-process writer ownership and native file confinement for RUN-01, split into reviewable children"
created_at: "2026-10-15T16:50:22Z"
has_completed_all_phases: false
---

# RUN-01: one writer per workspace, and a jail the native tools cannot leave

## Source and revision

- Plan identifier: `run01-workspace-confinement`.
- Source: fluzo-labs/fluzo#15 (RUN-01), task brief
  `/tmp/fluzo-task-T24.md`.
- Reviewed revision: issue body as read 2026-10-15; branch
  `feat/run01-workspace-confinement` at `6d2e6d4`.
- Design baseline: PRD v0.2 sections 21.1 and 21.2, Architecture
  decisions A02 and A05, fluzo-docs revision
  `60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Approval: pending. This plan is a design proposal; implementation of
  the split below is what the task brief authorizes.

## Goal

Two local Fluzo runtimes cannot write the same workspace, and every
agent-directed path operation resolves inside the assigned workspace root
or fails closed. A second runtime gets a typed owner diagnostic instead
of a second scheduler, and traversal, symlink, hard-link, special-file
and replacement-race fixtures all refuse rather than partially succeed.

## Scope and exclusions

In scope:

- Workspace identity from the resolved root and its filesystem identity,
  with a diagnostic fingerprint that does not expose the raw path.
- Cross-process exclusive writer ownership held for the whole ownership
  period, distinct from the SQLite per-transaction write lock, with
  quiescent release and stale-owner reconciliation.
- A native file confinement jail: root-relative fd-anchored resolution,
  traversal and symlink rejection, containment for new-file parents,
  special-file rejection, multiply-linked write rejection, protected
  internal paths, and replacement-race detection.

Excluded, with reasons:

- Process spawning, child environment, closed stdin, TTY and credential
  non-inheritance. Those are PRD 21.3 and A05's process half; there is
  no tool subprocess in this workspace yet, so the fixtures would be
  invented rather than exercised. Owned by the AGT chain.
- Git working-tree baseline classification (staged / unstaged /
  untracked). There is no Git interface anywhere in the workspace;
  building one is a separate surface. See RUN-01c.
- OS-enforced sandboxing. PRD 21.4 explicitly defers it, and PRD 21.2
  states the jail constrains native tool APIs only. Nothing here may be
  described as sandboxing an arbitrary program.
- A waiting queue for the second writer. PRD 21.2 allows "queued or
  blocked"; this delivers explicitly blocked with a typed outcome.
  Scheduling the queue is agent-chain work.

## Verified context

Observed facts:

- `Store` already anchors `.fluzodrive` from `/` with `O_NOFOLLOW |
  O_DIRECTORY | O_CLOEXEC` (`ANCHOR_FLAGS`), rejects a symlinked drive,
  and re-checks `dev`/`ino` of the held handle against the path after
  opening (`Store::anchor_drive_with`, `Store::verify_drive`).
- `anchor_class_in` generalizes that to one managed class directory under
  an already-anchored drive handle. This is the facility the jail builds
  on rather than a new mechanism.
- `Store::verify_database_file` rejects symlinks, non-files and
  `nlink != 1`. The multiply-linked rule already exists for the database.
- The SQLite write lock is per-transaction: `commit_unit` takes
  `BEGIN IMMEDIATE` and releases at commit. Between commits nothing is
  held, so it cannot provide PRD 21.2's ownership that "retain[s] it
  across dependent edits, verification commands, and approval waits".
  This is the gap RUN-01a fills.
- `Store::assert_live_store_is_free` and `connect_free_probe` exist but
  are used only by `restore`, and they prove momentary freedom, not
  retained ownership.
- `busy_timeout(Duration::ZERO)` is deliberate; `map_error` maps SQLite
  5/6 to `StoreError::Busy`.
- `test_diagnostics::expect_open` is the #81/#94 fixture discipline:
  bounded backoff on environment-flake classes only, full environment
  report on every failed attempt, semantic refusals never retried.
- No Git integration exists in any crate.
- Task brief claims the branch contains history purge #92. It does not:
  `purge.rs`/`history.rs` are absent from `origin/main` at `6d2e6d4`.
  Nothing in this plan depends on them.

Hypotheses to be proven by fixtures, not assumed:

- That `/proc/self/fd`-anchored `O_NOFOLLOW` opening is sufficient to
  close the check-to-use window for each component. Proven per-case by
  the replacement-race fixtures in P2.

## Decisions, dependencies, and risks

Decisions taken:

1. **Ownership is an OS file lock, not a database row.** A durable row
   alone cannot exclude another process: reading it and then writing is
   itself a race. An exclusive advisory lock on a file descriptor held by
   the owner for the whole ownership period is the primitive that
   actually excludes, and the kernel releases it on process death, which
   is what makes crashed owners recoverable at all. The durable record
   inside the file is diagnostic and reconciliation input, not the lock.
2. **Stale is not replaceable.** A record naming a holder that no longer
   exists is *reconciled*, with the reconciliation recorded, rather than
   silently stolen. A record naming a holder that still exists but is
   unreachable is refused. The distinction is the flock result, not a
   PID liveness guess, because PID liveness is exactly the "appears
   stale" inference PRD 21.2 forbids relying on.
3. **Release requires quiescence by construction.** Outstanding mutating
   operations are counted; `release` refuses while the count is
   non-zero. The count is held by tickets, so a leaked ticket blocks
   release rather than silently permitting it — the fail-closed
   direction.
4. **The jail is a separate object from the store.** The store owns
   `.fluzodrive`. Agent-directed paths are workspace files, which are a
   different trust domain. Sharing one handle type would let a jail bug
   reach managed storage.
5. **Protected names are refused by the jail, not hidden.** `.git`,
   `.fluzo`, `.fluzodrive` and the credential store return a typed
   `Protected` refusal naming the segment, so the caller can route to
   the settings or Git interface instead of guessing.

Risks:

- `/proc/self/fd` anchoring is Linux-specific. The workspace already
  fails closed off Linux (`Store::validate_root`), and the jail keeps
  that. No new platform claim is made.
- A lock file inside `.fluzodrive` becomes a new managed path. It must
  not be purged by DAT-02d/e work while held. Mitigation: it lives at a
  fixed name, is excluded from content scans, and the docs state it.
- Advisory locks are advisory. A non-Fluzo program can ignore them. The
  docs must say the contract excludes cooperating Fluzo processes only,
  consistent with STORAGE.md's existing coordinated-writer wording.

## Phases

### P1: Workspace identity and cross-process writer ownership

- Outcome: a second runtime attempting the same workspace is refused
  with a typed `HeldBy` naming the owning process and its identity
  fingerprint; the first can release only at a quiescent point; a
  killed owner is reconciled rather than silently replaced.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: DAT-01 store and drive anchoring (merged).
- Affected contracts: new `workspace` module in `fluzo-runtime`
  exporting `WorkspaceIdentity`, `WriterOwnership`, `OwnerRecord`,
  `OwnershipError`, `WriterTicket`, `ReleaseReceipt`. No existing
  signature changes.
- Acceptance criteria:
  - Identity comes from the anchored root's `dev`/`ino`, not from the
    path string; two paths to the same directory (one through a symlink)
    yield the same identity, and the diagnostic fingerprint contains no
    raw path.
  - Two independent owners in two processes cannot both hold; the second
    gets `HeldBy` with the first's pid and fingerprint, and dispatches
    nothing.
  - Ownership survives across unrelated store commits and is still held
    after them.
  - `release` with an outstanding ticket fails `NotQuiescent` naming the
    count, and the lock stays held.
  - Owner process killed: the lock is free, the stale record is
    reconciled and recorded, and the new owner's identity differs.
  - A record naming a live holder is never overwritten by a second
    acquire.
  - Read-only inspection of persisted history works while owned.
- Actions:
  - [x] Implement the identity and ownership module.
  - [x] Deterministic two-process fixtures using a child process and
    pipe barriers, not sleeps.
  - [x] Record verification and stop for review.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  workspace` — 19 passed, 1 ignored (the child role). Plus the
  workspace checks.
- Evidence:
  - Identity: a directory reached directly and through a symlink yield
    the same `WorkspaceIdentity` (dev/ino equal); the fingerprint is
    16 hex chars, stable, and appears nowhere in the workspace path.
  - Mutual exclusion: a child acquires and announces `READY <owner>` on
    a pipe. The parent's own acquire returns `HeldBy` naming that owner
    and a different pid, and leaves the lock file byte-identical.
  - Not the SQLite lock: the child then performs six `commit_unit`
    writes plus a checkpoint while holding ownership, and the parent is
    still refused by the same owner. The store write lock came and went
    underneath; the ownership did not move.
  - Quiescence: a live ticket makes `release` refuse with the count and
    the lock stays held (a second process is still excluded). After the
    ticket completes, release succeeds.
  - Uncertainty: `WriterTicket::uncertain` frees the ticket slot but
    keeps the refusal; release refuses on the first, second and third
    attempt. Uncertainty does not decay.
  - Crash: the crash-role child exits without releasing. `probe` reports
    `Abandoned`, and the next acquire records
    `reconciled_from = <dead owner>` with a different owner nonce.
  - Clean handoff: a released record carries `released_ms`; `probe`
    reports `Released` and the next acquire records
    `reconciled_from = None`.
  - Integrity: a symlinked lock path is refused with `Conflict` and
    creates nothing at the link target; a record naming a different
    workspace fingerprint is refused and the refusal quotes that
    fingerprint; a malformed record is refused as `CorruptRecord`.
  - `verify_still_owned` detects the drive directory being swapped for
    a different one under a live owner.
- Not covered: read-only history inspection from a second process while
  the owner holds the lock. The store opens with `busy_timeout = 0`, so
  a concurrent opener can meet `Busy` for reasons unrelated to this
  contract; testing it here would conflate the two. Left for the task
  runtime that owns the read path.
- Risks and recovery: the lock file is additive; removing the module
  leaves the store untouched.

### P2: Native file confinement jail

- Outcome: agent-directed read/create/edit against a relative path
  either resolves inside the assigned root through anchored handles or
  returns a typed refusal. Traversal, internal and external symlinks,
  magic-link-shaped replacements, hard links, device nodes, sockets,
  nonexistent parents and protected names all fail closed with a
  specific reason.
- Owning repository: fluzo-labs/fluzo, `fluzo-runtime`.
- Dependencies: P1 for the identity the jail reports; the existing
  `anchor_class_in` mechanism.
- Affected contracts: new `confinement` module exporting
  `WorkspaceJail`, `ConfinedFile`, `ConfinementError`. No existing
  signature changes.
- Acceptance criteria:
  - `..`, absolute paths, embedded NUL, empty and `.` components are
    refused before any filesystem call.
  - A symlink anywhere in the path, whether it points inside or outside,
    is refused; the jail never follows one.
  - Creating a file whose parent does not exist refuses rather than
    creating through an attacker-controlled parent.
  - A parent replaced between validation and open is detected: the
    `dev`/`ino` of the handle used differs from the validated one.
  - Writing to a file with `nlink > 1` is refused, so a hard link cannot
    carry an edit outside the root.
  - Device nodes, FIFOs and sockets are refused.
  - `.git`, `.fluzo` and `.fluzodrive` are refused as agent-directed
    targets with a typed `Protected` naming the segment.
  - Off-Linux and non-absolute roots fail closed.
- Verification: `cargo test -p fluzo-runtime --lib --locked --offline
  confinement` — 33 passed. Plus the workspace checks.
- Evidence, against each acceptance criterion above:
  - Absolute paths, `..`, `.`, empty components, an empty path and an
    embedded NUL are refused by parsing, before any filesystem call.
  - A symlink at the target is refused. A symlink that points back
    inside is refused too: the agent named a link, not the file, and a
    link is a pointer another process can replace.
  - A symlinked intermediate directory is refused in its deterministic
    race form: the jail is opened first, the test then replaces the
    intermediate with a link to `/etc`, and the later open refuses.
  - A create whose parent does not exist refuses and creates nothing.
  - A hard-linked target is refused with `SharedLink { links: 2 }` on
    either name, so a hard link cannot carry an edit outside the root.
  - FIFOs, sockets and other non-regular inodes are refused with
    `NotARegularFile`. A socket fails at open before a descriptor
    exists, so the label is refined by a stat on the already-refused
    decision; the refusal itself comes from the open.
  - `.git`, `.fluzo` and `.fluzodrive` are refused for read, write,
    create, remove and list, at the top level and at depth, with the
    offending segment named.
  - A rename cannot escape (traversal destination refused), cannot land
    on a protected name, and cannot replace a symlink destination.
  - A non-regular root and a missing root fail closed at jail open.
  - One combined inertness test runs every refusal class and asserts the
    directory snapshot is unchanged.
- Found while testing: `std::fs::File`'s derived `Debug` prints the
  path it was opened with, so `#[derive(Debug)]` on the jail types
  leaked the workspace path into any trace. Replaced with hand-written
  `Debug` impls reporting the descriptor number instead. PRD 21.1 asks
  for exactly that and the test caught it.
- Risks and recovery: additive module; no existing writer changes path.

### P3: User-change protection (RUN-01c, not in this increment)

- Outcome: a write to a file the runtime observed earlier is refused if
  the file changed since observation, so user edits are never silently
  overwritten.
- Why it is a separate child rather than part of this PR: it needs an
  observation/fingerprint contract and a Git working-tree baseline
  (staged vs unstaged vs untracked) that does not exist in this
  workspace. Implementing the fingerprint half without the Git
  classification half would claim protection it cannot attribute, and
  adding a Git interface here would widen this PR past review.
- Owning repository: fluzo-labs/fluzo.
- Dependencies: P2 (the jail is where the version check runs).
- Acceptance criteria: recorded on the child issue rather than here.
- Verification: on the child issue.

## Honest limits

- The jail constrains the native tool APIs implemented here. It is not
  an OS sandbox and does not stop a same-user program or an approved
  shell. PRD 21.2 says so explicitly and this delivery repeats it.
- Advisory file locks exclude cooperating processes. A program that does
  not take the lock is not excluded.
- Two-process fixtures use real child processes on one host. Multi-host
  or NFS-shared workspaces are out of scope and unverified.
- No agent, tool subprocess, or inference is involved; live inference
  is `not_run`.
- P3 is not implemented. AC 3 of #15 ("user changes remain protected")
  is therefore only partially addressed by this PR: the jail prevents
  writing outside the root and clobbering through hard links, but does
  not yet detect an external edit to a file inside the root.

## Next step

P1 and P2 are implemented and verified. Open one PR referencing both
children and `Ref: #15`, and leave P3 as an open child with its own
acceptance criteria. Human review is the gate; `implemented` is not
`accepted`.

## Evidence and tracking

- Parent issue: <https://github.com/fluzo-labs/fluzo/issues/15>.
- Children: created during the split step of this task; URLs recorded in
  the PR.
- GitHub issues remain the state authority; this document is a design
  snapshot, not a parallel board.
