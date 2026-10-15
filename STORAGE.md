# Repository store, durable journal and migrations (DAT-01, DAT-02c)

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

## Schema versions and the migration path (DAT-02c)

The schema is an explicit, ordered, forward-only list of steps in
`fluzo-runtime::migrations`. There is no automatic or implicit schema
change: every mutation of the schema goes through this path and is
recorded.

| Version | Shape |
| --- | --- |
| `1` | DAT-01 baseline: `store_meta`, `state`, `event`. |
| `2` | Adds the `artifact` table (DAT-02a). |
| `3` | Adds `state.updated_at` and the `state_kind_idx` index. |

`TARGET_VERSION` is the highest shape this build understands. A brand-new
store is created directly at the target, so it never needs a migration and
no backup is taken of an empty database. `MIGRATIONS` must stay contiguous
from `BASELINE_VERSION` to `TARGET_VERSION`; a focused test refuses a gap
or a step that migrates backwards. Down-migrations do not exist: the
backup and the explicit restore below are the recovery story.

DAT-02a landed its `artifact` table with a bootstrap marker update and a
comment pointing here. That stopgap is removed: the table is now a
registered `1 -> 2` step, so a store created before it also picks up the
audit trail and the backup, and the marker can no longer claim a shape the
database does not have.

Each step bumps the marker to its own `to_version`, never to
`TARGET_VERSION`. With more than one registered step a jump to the final
marker would let an intermediate shape claim to be the last one, and the
next step's backup verification would then reject its own snapshot. The
two-step registry is what surfaced this.

### Policy at open

`Store::open` keeps its DAT-01 signature and means `Migrate`.
`Store::open_with_policy` makes the intent explicit:

| Policy | Store below target | Store above target |
| --- | --- | --- |
| `ReadOnly` | `MigrationRequired`, nothing written | `SchemaTooNew` |
| `Migrate` | back up, verify, apply | `SchemaTooNew` |
| `Preserve` | opens as recorded, no migration | `SchemaTooNew` |

A store above the target is never partially interpreted, under any policy.
`ReadOnly` really does not write: it does not even create the audit table,
and the version check happens before any write is considered.

### Backup before mutation

Before a step touches the schema, `take_backup` writes a snapshot into
`.fluzodrive/backups/` (directory `0700`, file `0600`, name
`state-v<from>-to-v<to>-<n>.sqlite3`, first free index so a repeated
restore/re-migrate cycle never overwrites an earlier backup).

The snapshot uses `VACUUM INTO`, which reads through SQLite's own pager
while the source is locked and writes a consolidated, already-checkpointed
database. Committed rows still sitting only in the WAL are therefore
present in the backup; a focused test asserts the row is absent from the
live main file and present in the snapshot. The raw `sqlite3_backup_*` API
is not reachable here: `sqlx` keeps its connection handle crate-private
and this workspace forbids `unsafe`.

The backup is then reopened read-only and must pass integrity
(`PRAGMA quick_check`), carry the expected pre-migration version marker,
and still show the pre-migration shape (the column the step is about to
add must be absent). A backup that fails verification stops the migration
before any DDL runs; the schema is left untouched and the refusal is
recorded.

### Audit trail

Every attempt is recorded in `migration_run` inside the store itself and
read back with `Store::migration_log()`:
`from_version`, `to_version`, `state` (`running` / `succeeded` /
`failed` / `restored`), `backup_path`, `error`, `started_at`.

The ordering is what makes the crash boundary safe:

1. Back up and verify the untouched pre-migration database.
2. Commit the `running` row on its own, durably.
3. Apply the DDL, bump `schema_version` and mark the row `succeeded` in a
   single `BEGIN IMMEDIATE` transaction.

A crash anywhere in step 3 rolls the whole step back and leaves the
`running` row behind. The DDL and the version marker can never disagree.

### Refusal and recovery

On reopen, a `running` or `failed` audit row is refused whatever the
policy — `MigrationInterrupted` and `MigrationFailed` respectively —
before the version is even read. A half-migrated schema is never ridden
over by a read-only inspection. Nothing restores by itself: after a
failure the live store keeps its pre-migration shape and data, which a
focused test asserts directly.

`Store::restore(workspace_root, backup_path)` is the only recovery path
and the operator must name the backup, because restoring discards
everything the live store gained since the snapshot was taken. It accepts
a plain regular file with link count one inside this drive's own
`backups/` directory — no symlink, no hard link, no arbitrary path —
verifies it, proves the live store is not held by another writer
(`Busy` otherwise), stages the copy inside the drive, drops the stale
`-wal`/`-shm` of the store being replaced, and renames atomically. The
restored store is opened with `Preserve` so the operator sees exactly
what was restored rather than a silently re-migrated store, and the
restore is itself recorded as `restored`.

## Scope boundaries

Purge previews, quotas, artifact capture, cross-process ownership proof
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
cargo test -p fluzo-runtime --locked --offline migration_tests
```

Store covers: PRAGMA enforcement, private drive creation, atomic
state+event units, foreign-key whole-unit rollback, interrupted commit,
torn WAL recovery, writer-lock refusal and recovery, disk-full
fail-closed, symlink/file/hard-link drive rejection, and post-open drive
replacement detection.

```sh
cargo test -p fluzo-runtime --locked --offline artifact
```

Covers: finalization-before-reference ordering, byte-level independent
verification, truncation and redaction markers, omitted capture,
finalized immutability, reference-requires-finalized-content, whole-unit
atomicity with state and events, orphan detection and reclaim after an
interrupted commit, post-reference-commit durability, missing/torn/symlinked
content corruption reporting without repair, and id confinement.

Migration covers: registry contiguity and the absence of down-migrations,
fresh store at target with no migration recorded, `ReadOnly` refusal with
nothing written, `Preserve` opening an older store unchanged, newer-schema
refusal by every policy with the marker and data intact, migrate walking
the whole registered chain with a verified backup per step, WAL-inclusive
backup content, backup permissions, whole-step rollback on a failed step
with earlier committed steps preserved, refusal to reopen after a failure
with no automatic recovery, refusal to reopen after a real crash on the
durable running record (child abort fixture), exact pre-migration
restore, restore rejection of outside paths, symlinks, hard links and
corrupt snapshots, and restore refusal while another writer holds the
store.

## Storage quotas and admission guardrails (DAT-02b)

Quotas are per repository. Each store carries its own guard, sized from
that repository's `.fluzo` typed `storage` settings, so one project's
quota never reaches another project's store. Limits are never hardcoded
in the runtime; `QuotaLimits::from_storage` reads the same registry that
the configuration editor writes.

### Accounted scopes

| Class | What is counted | Setting | Default |
| --- | --- | --- | --- |
| `SessionData` | `state.sqlite3` plus its `-wal` and `-shm` side files, plus everything under `.fluzodrive/artifacts/` | `storage.session_max_bytes` | 10 GiB |
| `Diagnostics` | everything under `.fluzodrive/telemetry/` | `storage.diagnostic_max_bytes` | 2 GiB |
| free-space reserve | available bytes on the filesystem hosting `.fluzodrive/` | `storage.min_free_bytes` | 1 GiB |

Usage is recomputed from the filesystem on every check rather than
accumulated in memory or in a table. There is therefore no counter that
can drift from reality, and restart consistency is structural: the same
files measure the same amount before and after a reopen. The scan is
bounded by `MAX_ACCOUNTED_ENTRIES`; exceeding it is reported as a typed
error rather than silently under-counting.

`storage.auto_expire` remains `false` by default and is not read by the
quota path at all. Retention policy is a separate concern (PRD 22.2);
nothing here implements it.

### Admission outcomes

`Admission` is the typed result of a session-data check:

- `Admitted(status)` — the projected total stays below the limit.
- `Warning(status)` — utilization has reached 80%
  (`WARNING_THRESHOLD_PERCENT`) but the write still proceeds. The
  warning is observable through `Store::last_admission` before any
  refusal happens.
- `Refused(refusal)` — the write is rejected before it starts.

A refusal surfaces as `StoreError::QuotaReached { class, used, limit }`.
This is deliberately distinct from `StoreError::Full`, which means
SQLite's own page cap fired mid-write.

The guard runs before `BEGIN IMMEDIATE`, so a refusal leaves no partial
audit state: no row, no event, no artifact content. Each unit also
carries `DEFAULT_AUDIT_HEADROOM_BYTES` (64 KiB) of reserved room so the
refusal happens *before* the unit's own audit rows are at risk rather
than during them. Artifact content is admitted in
`finalize_artifact` before any byte lands on disk, not only when its
reference is committed.

### Diagnostics drop instead of refusing

Diagnostic capture never blocks ordinary execution. At the diagnostics
threshold `append_diagnostic` returns
`DiagnosticDisposition::Dropped { reason, status }` and the call
succeeds. Dropped records are counted, never silently discarded:
`Store::diagnostic_counters` reports `written_records`,
`written_bytes`, `dropped_records` and `dropped_bytes` for the current
run. A `Dropped` disposition is not an error; a returned `Err` from
that path is a genuine safety or I/O problem.

The reserve check applies to both classes: a low free-space reading
refuses session work and drops diagnostics rather than letting either
class push the filesystem toward exhaustion.

### Nothing is evicted

Reaching a threshold, elapsed time, or a reopen under saturated limits
removes nothing. Existing artifacts, journal rows and diagnostic
capture are retained byte for byte. Purge is a separate, explicitly
confirmed operation (#75) and is not implemented here.

### Attaching the guard

The guard is opt-in so existing callers are unaffected:

```rust
let store = Store::open_quotated(&workspace_root, QuotaLimits::from_storage(&settings.storage)).await?;
let store = Store::open_quotated_with_probe(&workspace_root, limits, MountedFileSystem).await?;
```

`Store::open` leaves the store unguarded; `append_diagnostic` on such a
store returns `StoreError::QuotaNotConfigured`, because the bounded
capture contract cannot be honoured without a guard. `Store::restore`
also returns an unguarded store: it is a recovery path that does not
read `.fluzo`, so the operator reopens with `open_quotated` to restore
quota enforcement. Free-space reads go through the `FileSystemProbe`
trait (`rustix::fs::statvfs`, using `f_bavail`), which the tests replace
with a fixed reading so no test depends on, or fills, a real disk.

### Verification

```sh
cargo test -p fluzo-runtime --locked --offline quota
```

Covers: defaults and typed-settings derivation, WAL side-file and
artifact accounting, class isolation, the 80% warning preceding
refusal, audit-headroom refusal, free-space reserve, diagnostics drop
with visible counters, saturation not blocking execution, mandatory
audit failure still blocking the task, refusal leaving no partial state,
artifact content admitted before it lands, restart recomputation and
stability, cross-repository independence, unsafe diagnostic record-name
rejection, and the no-eviction proof under pressure, elapsed time and
reopen.
