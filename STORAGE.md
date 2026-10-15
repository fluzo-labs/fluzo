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

## Focused checks

```sh
cargo test -p fluzo-runtime --locked --offline store
```

Covers: PRAGMA enforcement, private drive creation, atomic state+event
units, foreign-key whole-unit rollback, interrupted commit, torn WAL
recovery, writer-lock refusal and recovery, disk-full fail-closed,
symlink/file/hard-link drive rejection, and post-open drive replacement
detection.
