# Workspace identity, writer ownership and file confinement

Scope: RUN-01 ([#15](https://github.com/fluzo-labs/fluzo/issues/15)),
PRD 21.1 and 21.2, Architecture A02 and A05. Implementation:
`fluzo-runtime::workspace` and `fluzo-runtime::confinement`.

This is the layer that decides *which process may write this workspace*
and *where an agent-supplied path is allowed to land*. It sits below the
task runtime and above the store.

## Why the store lock is not ownership

`Store::commit_unit` opens `BEGIN IMMEDIATE` and releases it at commit.
Between two commits nothing is held. An approval wait falls in exactly
that gap: the agent has made a promise about an edit and is waiting for
a human, and during that wait a second Fluzo process could take the
same workspace and start editing the same files.

So ownership is a separate resource, deliberately held for the whole
period between acquiring the execution baseline and reaching a quiescent
checkpoint.

## Identity

`WorkspaceIdentity::resolve(root)` canonicalizes the root and then
re-opens it component by component from `/` with
`O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC`, reading `dev` and `ino` from
the descriptor it ends up holding.

Identity is the directory, not the spelling. Two paths that reach the
same directory agree; a path that was rewritten between calls does not
silently pass, because the anchored walk refuses to follow a component
that changed into a symlink.

`fingerprint()` is a 16-character hash of `dev:ino`. It is what goes in
records and traces. The resolved path is available through `root()` for
the runtime's own use and must not be put in a trace by default
(PRD 21.1).

## The writer lock

One file: `.fluzodrive/writer.lock`, mode `0600`.

The lock is `flock(LOCK_EX)` taken non-blockingly on a descriptor held
for the whole ownership period. That descriptor is the mechanism: a
durable row alone cannot exclude anyone, because reading it and then
writing it is itself a race. The kernel drops the lock when the owning
process dies, which is what makes a crashed owner recoverable at all.

The record inside the file is reconciliation input and a diagnostic, not
the lock:

```text
fluzo-writer-lock v1
pid=4242
acquired_ms=1760000000000
workspace=3f2a9c14be07d5a8
owner=9c4de0b17a2f6358
reconciled_from=none
released_ms=none
```

`owner` distinguishes one acquisition from another, because pids are
reused. `reconciled_from` is set when the acquirer found a record left
by a holder that no longer holds the lock — a crash, reconciled rather
than replaced. `released_ms` is written by a clean release, so the next
acquirer can tell a handoff from a crash.

### Refusal classes

| Result | Meaning |
| --- | --- |
| `HeldBy { holder }` | A live process holds it. The record names it. Nothing was written. |
| `HeldUnknown` | The lock is held but the record could not be read. Still refused: an unreadable holder is not an absent one. |
| `CorruptRecord { reason }` | The lock is free but the record is malformed, or names a different workspace. |
| `Conflict` | The lock path is a symlink or not a regular file. |
| `Inaccessible` | No root or no drive to work with. |

A refused acquire writes nothing. The test suite asserts the lock file is
byte-identical after a refusal.

### Release

`release()` refuses unless `outstanding == 0` and `uncertain == 0`, and
returns the ownership inside the refusal so the caller cannot end up
holding neither a lock nor a receipt.

- `begin_mutation()` hands out a `WriterTicket`. Dropping it decrements
  the count, so a forgotten ticket keeps blocking release rather than
  silently permitting it.
- `WriterTicket::uncertain()` frees the ticket slot but increments a
  count that is never decremented. "We do not know whether a write is
  in flight" is not "nothing is in flight". A workspace with an
  uncertain writer cannot be released and must be restarted.

On a successful release the `released_ms` marker is fsynced **before**
the lock is dropped, and the descriptor is closed there rather than left
to a deferred drop. A failed marker write is not reported as a clean
release.

### Stale is reconciled, never inferred

There is no PID-liveness check anywhere in this contract. A stale lock is
detected by the result of the lock attempt and the state of the record,
never by asking whether a process with some pid exists. PID reuse makes
liveness inference wrong in both directions, and a wrong "it is dead"
is how two writers end up in one workspace.

## The confinement jail

`WorkspaceJail::open(root)` holds an anchored descriptor for the root.
Every agent-supplied path is resolved against that descriptor one
component at a time, each opened with `O_NOFOLLOW`.

A string prefix check would be wrong here for the usual reason: between
the check and the open, another process can replace a component with a
symlink, and the open follows it. Opening each component relative to a
descriptor already held removes the window, because the descriptor
cannot be redirected by a later rename.

### Refused by the parser, before any filesystem call

Absolute paths, `..`, `.`, empty components, an empty path, an embedded
NUL, and any component named `.git`, `.fluzo` or `.fluzodrive` — at any
depth, so `sub/.git/hooks/pre-commit` is refused too. Those three names
carry executable hooks, operator configuration and durable state
respectively.

### Refused by the open

- A symlink at the target, or anywhere along the path. A symlink that
  points back inside is refused too: the agent named a link, not the
  file, and a link is a pointer another process can replace.
- A non-regular inode — FIFO, socket, device node. Opening one is not a
  file operation.
- A file with `nlink != 1`. Writing through a hard link changes every
  other name for the same inode, including names outside the workspace.
  Reported as `SharedLink { links }`.
- A create whose parent does not exist. The jail does not invent parent
  directories.

Renames use `renameat` between jail descriptors, so the destination
cannot be outside the jail, cannot be a protected name, and cannot
replace a symlink that was already there.

### Labels versus decisions

Linux reports `ENOTDIR` both for a plain non-directory component and for
a symlink met with `O_NOFOLLOW | O_DIRECTORY`. A socket fails at open
before there is a descriptor to inspect. In both cases the refusal comes
from the open flags; a follow-up `symlink_metadata` only chooses which
error name to return. A race on that stat cannot turn a refusal into a
success, because nothing was opened.

### No path in the diagnostics

`std::fs::File`'s derived `Debug` prints the path it was opened with.
Every type in this module that holds a descriptor therefore writes its own
`Debug` and reports the descriptor number instead, so a trace printed
from a `ConfinedFile` or a `WorkspaceJail` cannot leak the workspace
location (PRD 21.1).

## What this is not

- **Not an OS sandbox.** The jail confines the paths used through it. A
  process that opens files by absolute path directly is outside its
  reach, and PRD 21.2 says so plainly.
- **Advisory locking only.** `flock` excludes cooperating processes. A
  program that does not take the lock is not excluded.
- **Single host.** The fixtures run child processes on one machine.
  NFS-shared and multi-host workspaces are unverified.
- **Not wired to an agent yet.** There is no task runtime in this
  workspace, so these are tested primitives with no production caller.
  `session_attachment`-style wiring and the read path arrive with the
  execution story.
- **User-change protection is #97**, a separate child. Detecting an
  external edit to a file inside the root needs a Git working-tree
  baseline that does not exist here. Faking it with mtimes would report
  a false positive on every checkout and a false negative on
  same-second edits.

## Verification

```sh
cargo test -p fluzo-runtime --lib --locked --offline workspace
cargo test -p fluzo-runtime --lib --locked --offline confinement
```

The ownership fixtures use two real processes. A child acquires the
workspace and announces `READY <owner>` on a pipe; the parent acts on
that announcement rather than on a guessed delay. Child roles are
`hold`, `hold_and_commit` and `crash`, and the child role test is marked
`#[ignore]` so it only runs when spawned.
