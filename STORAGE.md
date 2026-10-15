# Repository store and durable journal (DAT-01)

`fluzo-runtime::store` implements the storage foundation required by PRD
sections 21-24 and Architecture decision A03. It is the only component
that writes `.fluzodrive/`.

## Location and protection

- Journal path: `.fluzodrive/state.sqlite3` inside the Git working tree.
  `.fluzodrive/` is git-ignored and must never be committed, together with
  its `-wal`/`-shm` side files.
- The drive directory is opened through fd-anchored traversal from `/`
  with `O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC`, created with mode `0700`
  when absent, and rejected when it is a symlink or a non-directory. The
  SQLite file itself is rejected when it is a symlink or has a link count
  other than one. Before every commit the stored parent handle is compared
  against the on-disk path by `dev`/`ino`; a replaced or removed drive is
  refused (`Conflict`) rather than written through. This mirrors the
  `.fluzo` configuration writer's fail-closed discipline.
- The coordinated-writer contract covers this tool's own processes.
  Arbitrary external editors and network filesystems are outside it.

## Durability configuration

Opened with, and verified at open time by reading each PRAGMA back:

| PRAGMA | Required value | Failure |
| --- | --- | --- |
| `journal_mode` | `wal` | `PragmaNotEnforced`, open refused |
| `synchronous` | `2` (`FULL`) | `PragmaNotEnforced`, open refused |
| `foreign_keys` | `1` (`ON`) | `PragmaNotEnforced`, open refused |

`busy_timeout` is zero: lock contention surfaces immediately as `Busy`
instead of blocking the caller.

## Atomic logical units

`Store::commit_unit(updates, events)` runs one `BEGIN IMMEDIATE`
transaction that applies all state upserts and appends all journal records.
There is no partial visibility: a failing row (for example a journal
record referencing a state id that does not exist) discards the entire
unit and leaves prior data untouched. Journal rows carry a monotonic
`seq`; readers page with `events_after(seq, limit)` for cursor handoff.

## Crash and failure semantics (A03)

Recovery either replays complete durable work or refuses; it never
half-applies. Covered by deterministic crash fixtures:

- Interrupted commit: a child process aborts inside an open transaction.
  On reopen the interrupted unit is invisible; units committed before the
  interruption survive.
- Torn WAL tail: truncating the WAL replays only complete,
  checksum-valid transactions. The last torn transaction is refused and
  `PRAGMA quick_check` still reports `ok`.
- Writer-lock loss: while another process holds the write lock, commits
  fail with `Busy` (no queue, no timeout). After the holder dies, a fresh
  open recovers and commits normally.
- Disk full: simulated with `PRAGMA max_page_count` so SQLite returns the
  real `SQLITE_FULL` error class. The failed unit is fully rolled back and
  prior state remains readable. A physically full filesystem was not
  tested; the SQLite error boundary is identical.

Errors are machine-readable: `UnsupportedPath`, `Inaccessible`,
`Conflict`, `PragmaNotEnforced`, `Busy`, `Full`, `Corrupt`,
`Constraint`, `Io`. SQLite extended result codes are mapped by their
primary class (low byte).

## Scope boundaries

Migration backup/restore, purge previews, cross-process ownership proof
and task/attempt/operation state machines are later DAT/EXEC child work
and are not implemented or implied here. No credentials, provider
endpoints or conversation content belong in the journal.

## Artifacts (DAT-02a)

Artifact bytes live under the managed root `.fluzodrive/artifacts/` and
are written only through the store-owned, drive-anchored writer
(`fluzo-runtime::artifact`). No other component writes there. Artifact
ids are validated to a single safe path component (no separators, no
leading dot, bounded length); content files are opened `O_NOFOLLOW`, so
symlinks are never followed in either direction.

**Finalization order (durability-separated).** Step 1,
`finalize_artifact`: redact, bound, write a staging file `0600`, fsync,
rename into place and fsync the directory. The finalized file is
complete, size-recorded and immutable; a duplicate finalize or a
re-capture of a committed id is refused. Step 2,
`commit_artifact_reference`: re-checks the finalized bytes (present and
hash-matching) and then inserts the `artifact` reference row together
with the owning state updates and journal events in one `BEGIN
IMMEDIATE` unit. A reference can never commit without its finalized
content, and reference/state/event are atomic (foreign key on
`artifact.state_id`).

**Bounded capture.** The finite per-output limit is 1 MiB
(`MAX_ARTIFACT_BYTES`). Larger inputs are stored as the leading prefix
and marked `Truncated`. Redaction of caller-supplied secret literals
happens before persistence in both content and record fields and is
marked `Redacted`. Intentionally absent content is recorded as `Omitted`
with no content file. No marker is ever implied silently.

**Verified crash semantics.** A crash after the content write but before
the reference commit leaves a detectable, reclaimable orphan
(`detect_orphaned_artifacts`); never a dangling reference. A crash
after the reference commit implies the finalized content is present and
verifies. A torn or removed content file behind a committed reference
reports `Corrupt` with a reason and is never silently repaired,
re-finalized or returned as valid (`verify_artifact`,
`read_artifact_content`). The persisted record (id, owning state, kind,
provenance, size, SHA-256, flags) supports shared-reference analysis for
purge (DAT-02d) without scanning bytes.

**Scope.** Reclamation of orphans, quotas and purge commands are
DAT-02b/#75; the versioned migration framework is DAT-02c/#74. Real
model/shell capture arrives with later EXEC work; the contract here is
proven with fixture producers.

## Focused checks

```sh
cargo test -p fluzo-runtime --locked --offline store
```

Covers: PRAGMA enforcement, private drive creation, atomic state+event
units, foreign-key whole-unit rollback, interrupted commit, torn WAL
recovery, writer-lock refusal and recovery, disk-full fail-closed,
symlink/file/hard-link drive rejection, and post-open drive replacement
detection.

```sh
cargo test -p fluzo-runtime --locked --offline artifact
```

Covers: finalization-before-reference ordering, byte-level independent
verification, truncation and redaction markers, omitted capture,
finalized immutability, reference-requires-finalized-content, whole-unit
atomicity with state and events, orphan detection and reclaim after an
interrupted commit, post-reference-commit durability, missing/torn/symlinked
content corruption reporting without repair, and id confinement.
