//! DAT-01 repository store: protected `.fluzodrive` root, durable SQLite
//! journal and atomic state/event commits.
//!
//! Contract (PRD sections 21-24, Architecture decision A03): the journal
//! runs with `journal_mode=WAL`, `synchronous=FULL` and `foreign_keys=ON`,
//! verified at open time and refused fail-closed when not enforceable. State
//! and events for one logical unit commit in a single `BEGIN IMMEDIATE`
//! transaction so no reader can observe a half-applied unit. The drive
//! directory reuses the configuration writer's fail-closed discipline:
//! fd-anchored traversal, symlink rejection, hard-link rejection and a
//! parent identity recheck before each commit.
//!
//! DAT-02c adds the schema migration path in [`crate::migrations`]: explicit
//! ordered versioned steps applied only under an explicit
//! [`MigrationPolicy`], a checkpoint-consistent backup taken and verified
//! before any schema mutation, durable migration audit rows in this store,
//! and an explicit [`Store::restore`] for operator recovery.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, Row};

use crate::quota::{
    Admission, DIAGNOSTICS_DIRECTORY, DiagnosticDisposition, FileSystemProbe, MountedFileSystem,
    QuotaClass, QuotaLedger, QuotaLimits, QuotaStatus,
};

pub const DRIVE_DIRECTORY: &str = ".fluzodrive";
pub const DATABASE_FILE: &str = "state.sqlite3";
pub(crate) const SCHEMA_VERSION: &str = "5";

use crate::migrations::{self};

pub use crate::migrations::{
    BACKUP_DIRECTORY, BASELINE_VERSION, MIGRATIONS, MigrationPolicy, MigrationRecord,
    MigrationState, MigrationStep, TARGET_VERSION,
};

// O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC, matching the configuration writer.
pub(crate) const ANCHOR_FLAGS: i32 = 0x20000 | 0x10000 | 0x800;
// O_NOFOLLOW | O_CLOEXEC for managed content files (no O_DIRECTORY).
pub(crate) const NOFOLLOW_CLOEXEC: i32 = 0x20000 | 0x800;

/// Upper bound for a diagnostic record name. Keeps the managed directory
/// flat and the names inspectable.
const MAX_DIAGNOSTIC_NAME_BYTES: usize = 128;

/// Diagnostic record names are flat, dot-safe and never hidden files.
fn is_safe_diagnostic_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_DIAGNOSTIC_NAME_BYTES
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// Lower bound on the bytes a commit unit adds to the journal: the
/// row content it carries. Deliberately conservative in the direction
/// that refuses earlier, which is the safe direction for an admission
/// threshold.
fn unit_bytes(updates: &[StateUpdate], events: &[EventRecord]) -> u64 {
    let updates: u64 = updates
        .iter()
        .map(|update| (update.id.len() + update.kind.len() + update.payload.len()) as u64)
        .sum();
    let events: u64 = events
        .iter()
        .map(|event| (event.state_id.len() + event.kind.len() + event.payload.len()) as u64)
        .sum();
    updates.saturating_add(events)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreError {
    UnsupportedPath,
    Inaccessible,
    Conflict,
    PragmaNotEnforced,
    Busy,
    Full,
    Corrupt,
    Constraint,
    Io,
    /// The store records a schema version above `TARGET_VERSION`. Never
    /// partially interpreted: the open is refused.
    SchemaTooNew,
    /// The store is below the target version and the policy forbids migrating.
    MigrationRequired,
    /// The pre-migration backup did not survive verification, so the
    /// migration was not attempted and the schema is unchanged.
    BackupVerificationFailed,
    /// A migration step failed. The store is intact but the recorded attempt
    /// is not terminal, so reopen refuses until an operator acts.
    MigrationFailed,
    /// A recorded migration never reached a terminal state (crash boundary).
    /// The pre-migration backup is the recovery path.
    MigrationInterrupted,
    /// DAT-02b: this repository's quota for the named class is reached, so
    /// the work was refused before starting. Distinct from `Full`, which is
    /// SQLite reporting a page-cap exhaustion during a write.
    QuotaReached {
        class: QuotaClass,
        used: u64,
        limit: u64,
    },
    /// The bounded local diagnostic capture contract needs an attached
    /// quota guard; this store was opened without one.
    QuotaNotConfigured,
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::UnsupportedPath => "unsupported store path",
            Self::Inaccessible => "store is inaccessible",
            Self::Conflict => "store path conflicts with a safe owner",
            Self::PragmaNotEnforced => "required durability PRAGMA not enforced",
            Self::Busy => "store write lock is held elsewhere",
            Self::Full => "store storage is full",
            Self::Corrupt => "store is corrupt",
            Self::Constraint => "store constraint violated",
            Self::Io => "store I/O failure",
            Self::SchemaTooNew => "store schema is newer than this build supports",
            Self::MigrationRequired => "store schema migration is required",
            Self::BackupVerificationFailed => "pre-migration backup failed verification",
            Self::MigrationFailed => "schema migration failed",
            Self::MigrationInterrupted => "schema migration did not finish",
            Self::QuotaReached { .. } => "repository storage quota reached",
            Self::QuotaNotConfigured => "store has no quota guard attached",
        };
        formatter.write_str(text)
    }
}

/// One current-state row written as part of a logical unit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateUpdate {
    pub id: String,
    pub kind: String,
    pub payload: String,
}

/// One durable journal record referencing a state row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventRecord {
    pub state_id: String,
    pub kind: String,
    pub payload: String,
}

/// A journal record read back with its monotonic sequence number.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredEvent {
    pub seq: i64,
    pub state_id: String,
    pub kind: String,
    pub payload: String,
}

#[derive(Debug)]
pub struct Store {
    pub(crate) connection: sqlx::SqliteConnection,
    pub(crate) drive_directory: PathBuf,
    pub(crate) drive_handle: File,
    /// Schema version this store was opened at. `commit_unit` needs it
    /// because `updated_at` only exists from v3 on, and a store opened under
    /// `Preserve` legitimately sits below the migration target.
    schema_version: u32,
    quota: Option<QuotaLedger>,
    last_admission: Option<Admission>,
    diagnostics: DiagnosticCounters,
}

/// Visible degradation counters for bounded local diagnostic capture.
/// Dropped records are counted, never silently discarded.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticCounters {
    pub written_records: u64,
    pub written_bytes: u64,
    pub dropped_records: u64,
    pub dropped_bytes: u64,
}

impl Store {
    /// Opens (creating when absent) the workspace journal under a protected
    /// `.fluzodrive` directory, applying the explicit forward migration path.
    /// Fails closed before any write when the directory, the database file or
    /// the parent identity is unsafe.
    pub async fn open(workspace_root: &Path) -> Result<Self, StoreError> {
        Self::open_with_policy(workspace_root, MigrationPolicy::Migrate).await
    }

    /// Opens the store under an explicit migration policy. Migrations are
    /// applied only here, only by the store owner, and only forward.
    pub async fn open_with_policy(
        workspace_root: &Path,
        policy: MigrationPolicy,
    ) -> Result<Self, StoreError> {
        Self::open_inner(workspace_root, policy, None, MountedFileSystem).await
    }

    /// Opens the journal with this repository's storage quota attached.
    /// Limits come from the repository's `.fluzo` typed settings, so one
    /// project's quota never reaches another project's store.
    pub async fn open_quotated(
        workspace_root: &Path,
        limits: QuotaLimits,
    ) -> Result<Self, StoreError> {
        Self::open_inner(
            workspace_root,
            MigrationPolicy::Migrate,
            Some(limits),
            MountedFileSystem,
        )
        .await
    }

    /// As `open_quotated`, with an explicit migration policy.
    pub async fn open_quotated_with_policy(
        workspace_root: &Path,
        limits: QuotaLimits,
        policy: MigrationPolicy,
    ) -> Result<Self, StoreError> {
        Self::open_inner(workspace_root, policy, Some(limits), MountedFileSystem).await
    }

    /// As `open_quotated`, with an injectable filesystem probe so the
    /// free-space reserve path is deterministic without filling a disk.
    pub async fn open_quotated_with_probe(
        workspace_root: &Path,
        limits: QuotaLimits,
        probe: impl FileSystemProbe + Send + Sync + 'static,
    ) -> Result<Self, StoreError> {
        Self::open_inner(
            workspace_root,
            MigrationPolicy::Migrate,
            Some(limits),
            probe,
        )
        .await
    }

    async fn open_inner(
        workspace_root: &Path,
        policy: MigrationPolicy,
        limits: Option<QuotaLimits>,
        probe: impl FileSystemProbe + Send + Sync + 'static,
    ) -> Result<Self, StoreError> {
        Self::validate_root(workspace_root)?;
        let drive_directory = workspace_root.join(DRIVE_DIRECTORY);
        let drive_handle = Self::anchor_drive_with(&drive_directory, true)?;
        let database_path = drive_directory.join(DATABASE_FILE);
        Self::verify_database_file(&database_path)?;

        let options = SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::ZERO);
        let connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .map_err(|error| {
                #[cfg(test)]
                record_connect_error(&error);
                // A connection-time failure used to erase the driver's
                // error class entirely: a transient BUSY or a FULL device
                // surfaced as a generic `Inaccessible`, which made CI
                // flakes undiagnosable. Transient classes now surface
                // distinctly; unknown I/O still fails closed as
                // `Inaccessible`.
                match map_error(error) {
                    StoreError::Io => StoreError::Inaccessible,
                    other => other,
                }
            })?;
        // The quota ledger is built before the Self literal so the store
        // carries it, but this introduces no fallible step outside the
        // close-guarded block below: QuotaLedger::new only stores its
        // drive path, limits and probe. verify_pragmas deliberately stays
        // inside that block rather than being called here, so a pragma
        // failure still closes the connection deterministically instead
        // of deferring the real close to a background thread and leaving
        // the WAL index lock held.
        let quota = limits.map(|limits| QuotaLedger::new(drive_directory.clone(), limits, probe));
        let mut store = Self {
            connection,
            drive_directory,
            drive_handle,
            schema_version: 0,
            quota,
            last_admission: None,
            diagnostics: DiagnosticCounters::default(),
        };
        // Every post-connect failure path closes the connection
        // deterministically. Dropping a sqlx connection defers the real
        // close to a background thread, and a lingering connection still
        // holding the WAL index lock makes the next busy_timeout=0 open
        // (e.g. a restore's free-store probe) fail with SQLITE_BUSY.
        let outcome = async {
            Self::verify_pragmas(&mut store.connection).await?;
            store.prepare_schema(policy).await?;
            store.schema_version = migrations::recorded_version(&mut store.connection)
                .await?
                .unwrap_or(0);
            Ok::<(), StoreError>(())
        }
        .await;
        if let Err(error) = outcome {
            let _ = store.connection.close().await;
            return Err(error);
        }
        Ok(store)
    }

    /// Rejects paths this store will not operate on. Shared by open and
    /// restore so both apply the identical discipline.
    fn validate_root(workspace_root: &Path) -> Result<(), StoreError> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) || !workspace_root.is_absolute()
        {
            return Err(StoreError::UnsupportedPath);
        }
        if workspace_root
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(StoreError::UnsupportedPath);
        }
        Ok(())
    }

    /// Brings the schema to the state the policy allows, refusing closed
    /// otherwise.
    ///
    /// A brand-new store is created directly at the target version, so it
    /// never needs a migration and no backup is taken of an empty database.
    /// An existing store is compared against the target: above it is always
    /// refused, below it is migrated only under `Migrate`.
    async fn prepare_schema(&mut self, policy: MigrationPolicy) -> Result<(), StoreError> {
        // No marker table at all means this is a brand-new store rather than
        // an unrecognised one, so build the current shape and stop.
        if !migrations::has_table(&mut self.connection, "store_meta").await? {
            return Self::create_baseline(&mut self.connection).await;
        }
        // A previous attempt that never reached a terminal state means the
        // schema may be half-migrated. Refuse rather than guess, whatever
        // the policy: no read-only inspection may ride over a broken step.
        // The audit table is only read when it already exists, because a
        // read-only policy must not write at all.
        if migrations::has_table(&mut self.connection, "migration_run").await?
            && let Some(record) = migrations::latest_run(&mut self.connection).await?
        {
            match record.state {
                MigrationState::Running => return Err(StoreError::MigrationInterrupted),
                MigrationState::Failed => return Err(StoreError::MigrationFailed),
                MigrationState::Succeeded | MigrationState::Restored => {}
            }
        }
        let current = migrations::recorded_version(&mut self.connection)
            .await?
            .ok_or(StoreError::Corrupt)?;
        if current > migrations::TARGET_VERSION {
            return Err(StoreError::SchemaTooNew);
        }
        if current == migrations::TARGET_VERSION {
            return Ok(());
        }
        match policy {
            MigrationPolicy::ReadOnly => Err(StoreError::MigrationRequired),
            MigrationPolicy::Preserve => Ok(()),
            MigrationPolicy::Migrate => {
                // Only now, and only because we are about to migrate, is the
                // audit table created. It records history and is deliberately
                // not part of the versioned schema evolution, so an older
                // store can be audited before it is migrated.
                migrations::ensure_audit_table(&mut self.connection).await?;
                self.run_migrations(current).await
            }
        }
    }

    /// Applies the registered forward steps from `current` to the target,
    /// one at a time. Each step is backed up and verified before its DDL, and
    /// its start is committed durably before the DDL runs, so a crash leaves
    /// a non-terminal record that reopen refuses to ignore.
    async fn run_migrations(&mut self, mut current: u32) -> Result<(), StoreError> {
        while current < migrations::TARGET_VERSION {
            let Some(step) = migrations::step_for(current) else {
                // No registered step for this version: refuse rather than
                // guess at a path forward.
                return Err(StoreError::MigrationFailed);
            };
            self.verify_drive()?;
            let backup = migrations::take_backup(
                &mut self.connection,
                &self.drive_directory,
                step.from_version(),
                step.to_version(),
            )
            .await?;
            if let Err(error) = migrations::verify_backup(&backup, step.from_version(), step).await
            {
                // The schema was never touched. Record the refusal so the
                // operator can see why the store will not come up.
                let _ = migrations::record_run(
                    &mut self.connection,
                    step.from_version(),
                    step.to_version(),
                    MigrationState::Failed,
                    Some(&backup.to_string_lossy()),
                    Some("backup verification failed; migration not attempted"),
                )
                .await;
                return Err(error);
            }
            let run_id = migrations::record_run(
                &mut self.connection,
                step.from_version(),
                step.to_version(),
                MigrationState::Running,
                Some(&backup.to_string_lossy()),
                None,
            )
            .await?;
            #[cfg(test)]
            if migrations::CRASH_AFTER_RUNNING_RECORD.load(std::sync::atomic::Ordering::SeqCst) {
                // The test harness captures the child's streams and an abort
                // discards them, so the crash point is announced through a
                // file the parent can read back.
                let _ = fs::write(
                    self.drive_directory.join(migrations::CRASH_MARKER_FILE),
                    b"crashed on the durable running record",
                );
                std::process::abort();
            }
            match self.apply_step_and_commit(step, run_id).await {
                Ok(()) => current = step.to_version(),
                Err(error) => {
                    let reason = error.to_string();
                    let _ = migrations::finish_run(
                        &mut self.connection,
                        run_id,
                        MigrationState::Failed,
                        Some(&reason),
                    )
                    .await;
                    // The underlying driver error belongs in the audit row;
                    // the caller only needs to know the migration failed.
                    return Err(StoreError::MigrationFailed);
                }
            }
        }
        Ok(())
    }

    /// Applies one step, bumps the version marker and marks the run
    /// successful in a single transaction: the step either fully lands or
    /// not at all.
    async fn apply_step_and_commit(
        &mut self,
        step: MigrationStep,
        run_id: i64,
    ) -> Result<(), StoreError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(map_error)?;
        migrations::apply(&mut transaction, step).await?;
        sqlx::query("UPDATE store_meta SET value = ?1 WHERE key = 'schema_version'")
            // The step's own target, never the overall TARGET_VERSION: with
            // more than one registered step a jump to the final marker would
            // leave the intermediate shape claiming to be the last one.
            .bind(step.to_version().to_string())
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        sqlx::query("UPDATE migration_run SET state = ?2, error = NULL WHERE id = ?1")
            .bind(run_id)
            .bind(MigrationState::Succeeded.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        transaction.commit().await.map_err(map_error)
    }

    /// The schema version this store is recorded at.
    pub async fn schema_version(&mut self) -> Result<u32, StoreError> {
        Ok(self.schema_version)
    }

    /// Test-only handle to the live connection, for fixtures that must hold
    /// the write lock or take a backup through the migration helpers.
    #[cfg(test)]
    pub(crate) fn connection_for_test(&mut self) -> &mut sqlx::SqliteConnection {
        &mut self.connection
    }

    /// Test-only alias for the production deterministic close.
    #[cfg(test)]
    pub(crate) async fn close_for_test(self) -> Result<(), StoreError> {
        self.close().await
    }

    /// Closes the connection synchronously instead of deferring it.
    ///
    /// Dropping a sqlx connection defers the real close to a background
    /// thread, so a process that reopens the same store right after a
    /// parent `drop` can hit SQLITE_BUSY on the WAL index. Any caller
    /// that opens a store in order to hand it to a later open -- a
    /// short-lived CLI, a fixture handing work to a child -- must close
    /// it explicitly rather than dropping it.
    pub async fn close(self) -> Result<(), StoreError> {
        self.connection.close().await.map_err(map_error)
    }

    /// The durable migration audit trail, oldest first.
    pub async fn migration_log(&mut self) -> Result<Vec<MigrationRecord>, StoreError> {
        migrations::migration_log(&mut self.connection).await
    }

    /// Explicitly restores the live store from a pre-migration backup taken
    /// by this module, returning it to the exact state captured in that
    /// backup.
    ///
    /// Nothing calls this automatically. A failed or interrupted migration
    /// only refuses to open and reports the backup path; the operator must
    /// name the backup to restore. Restoring discards everything the live
    /// store gained after the backup was taken, which is why it is never
    /// inferred.
    ///
    /// The backup must be a plain regular file inside this drive's own
    /// `backups/` directory: no symlinks, no hard links, no arbitrary path.
    /// It is verified before anything is replaced, and the replacement is a
    /// staged rename inside the drive so it is atomic.
    pub async fn restore(workspace_root: &Path, backup_path: &Path) -> Result<Self, StoreError> {
        Self::validate_root(workspace_root)?;
        let drive_directory = workspace_root.join(DRIVE_DIRECTORY);
        let drive_handle = Self::anchor_drive_with(&drive_directory, true)?;
        let backups = migrations::ensure_backup_directory(&drive_directory)?;
        if backup_path.parent() != Some(backups.as_path()) {
            return Err(StoreError::UnsupportedPath);
        }
        let metadata = fs::symlink_metadata(backup_path).map_err(|_| StoreError::Inaccessible)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StoreError::Conflict);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(StoreError::Conflict);
            }
        }
        let restored_version = migrations::verify_restore_backup(backup_path).await?;

        // Prove the live store is not held by another writer before we
        // replace it underneath them.
        Self::assert_live_store_is_free(&drive_directory).await?;

        let database = drive_directory.join(DATABASE_FILE);
        let staging = drive_directory.join(format!("{DATABASE_FILE}.restore"));
        fs::copy(backup_path, &staging).map_err(|_| StoreError::Io)?;
        let staged = OpenOptions::new()
            .write(true)
            .open(&staging)
            .map_err(|_| StoreError::Io)?;
        staged.sync_all().map_err(|_| StoreError::Io)?;
        drop(staged);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))
                .map_err(|_| StoreError::Io)?;
        }
        // Any leftover WAL/shared-memory files belong to the store being
        // replaced, not to the snapshot, so they must not survive the swap.
        let _ = fs::remove_file(drive_directory.join(format!("{DATABASE_FILE}-wal")));
        let _ = fs::remove_file(drive_directory.join(format!("{DATABASE_FILE}-shm")));
        fs::rename(&staging, &database).map_err(|_| StoreError::Io)?;

        // `Preserve` so the operator sees exactly what was restored rather
        // than a silently re-migrated store.
        let mut store = Self::open_with_policy(workspace_root, MigrationPolicy::Preserve).await?;
        store.verify_drive()?;
        migrations::ensure_audit_table(&mut store.connection).await?;
        migrations::record_run(
            &mut store.connection,
            restored_version,
            restored_version,
            MigrationState::Restored,
            Some(&backup_path.to_string_lossy()),
            None,
        )
        .await?;
        drop(drive_handle);
        Ok(store)
    }

    /// Takes and releases the write lock on the live database to prove no
    /// other writer holds it. Returns `Busy` rather than replacing a store
    /// that is in use.
    async fn assert_live_store_is_free(drive_directory: &Path) -> Result<(), StoreError> {
        let database = drive_directory.join(DATABASE_FILE);
        if fs::symlink_metadata(&database).is_err() {
            return Ok(());
        }
        let options = SqliteConnectOptions::new()
            .filename(&database)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::ZERO);
        let mut connection = Self::connect_free_probe(&options).await?;
        let locked = match connection.begin_with("BEGIN IMMEDIATE").await {
            Ok(transaction) => transaction.rollback().await.is_err(),
            Err(_) => true,
        };
        drop(connection);
        if locked {
            return Err(StoreError::Busy);
        }
        Ok(())
    }

    /// Connect for the free-store probe, retrying only the transient
    /// connect-time BUSY that appears while a dying connection's WAL
    /// index lock is still being released. A persistently busy connect
    /// means the store is in use: refuse with `Busy`, never proceed.
    async fn connect_free_probe(
        options: &SqliteConnectOptions,
    ) -> Result<sqlx::SqliteConnection, StoreError> {
        let max_attempts = 4;
        let mut delay = Duration::from_millis(50);
        for attempt in 1..=max_attempts {
            match sqlx::SqliteConnection::connect_with(options).await {
                Ok(connection) => return Ok(connection),
                Err(error) => match map_error(error) {
                    StoreError::Busy if attempt < max_attempts => {
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                    StoreError::Busy => return Err(StoreError::Busy),
                    _ => return Err(StoreError::Inaccessible),
                },
            }
        }
        Err(StoreError::Busy)
    }

    /// Commits state updates and journal records for one logical unit in a
    /// single `BEGIN IMMEDIATE` transaction. Either every row lands or none
    /// does; a foreign-key violation discards the whole unit.
    pub async fn commit_unit(
        &mut self,
        updates: &[StateUpdate],
        events: &[EventRecord],
    ) -> Result<(), StoreError> {
        self.verify_drive()?;
        self.admit_session_work(updates, events)?;
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(map_error)?;
        let now = migrations::epoch_seconds();
        for update in updates {
            if self.schema_version >= 3 {
                sqlx::query(
                    "INSERT INTO state (id, kind, payload, revision, updated_at)
                     VALUES (?1, ?2, ?3, 1, ?4)
                     ON CONFLICT(id) DO UPDATE SET kind = excluded.kind,
                         payload = excluded.payload, revision = state.revision + 1,
                         updated_at = excluded.updated_at",
                )
                .bind(&update.id)
                .bind(&update.kind)
                .bind(&update.payload)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(map_error)?;
            } else {
                sqlx::query(
                    "INSERT INTO state (id, kind, payload, revision) VALUES (?1, ?2, ?3, 1)
                     ON CONFLICT(id) DO UPDATE SET kind = excluded.kind,
                         payload = excluded.payload, revision = state.revision + 1",
                )
                .bind(&update.id)
                .bind(&update.kind)
                .bind(&update.payload)
                .execute(&mut *transaction)
                .await
                .map_err(map_error)?;
            }
        }
        for event in events {
            sqlx::query("INSERT INTO event (state_id, kind, payload) VALUES (?1, ?2, ?3)")
                .bind(&event.state_id)
                .bind(&event.kind)
                .bind(&event.payload)
                .execute(&mut *transaction)
                .await
                .map_err(map_error)?;
        }
        transaction.commit().await.map_err(map_error)
    }

    /// DAT-02b session admission. Runs strictly before `BEGIN IMMEDIATE`
    /// so a refusal leaves no partial audit state: nothing has been
    /// written when the caller sees the typed refusal. The audit headroom
    /// is what makes the refusal happen *before* the unit's own audit
    /// rows are at risk rather than during them.
    pub(crate) fn admit_session_work(
        &mut self,
        updates: &[StateUpdate],
        events: &[EventRecord],
    ) -> Result<(), StoreError> {
        self.admit_session_bytes(unit_bytes(updates, events))
    }

    /// Shared session admission for any write that grows this repository's
    /// session-data class, whether it is a journal unit or artifact bytes.
    pub(crate) fn admit_session_bytes(&mut self, incoming: u64) -> Result<(), StoreError> {
        let Some(ledger) = self.quota.as_ref() else {
            return Ok(());
        };
        let admission = ledger.admit_session(incoming)?;
        self.last_admission = Some(admission);
        match admission {
            Admission::Refused(refusal) => Err(StoreError::QuotaReached {
                class: refusal.status.class,
                used: refusal.status.used_bytes,
                limit: refusal.status.limit_bytes,
            }),
            Admission::Warning(_) | Admission::Admitted(_) => Ok(()),
        }
    }

    /// Live utilization of one accounted class, or `None` when this store
    /// was opened without a quota guard.
    pub fn quota_status(&self, class: QuotaClass) -> Result<Option<QuotaStatus>, StoreError> {
        self.quota
            .as_ref()
            .map(|ledger| ledger.status(class))
            .transpose()
    }

    /// The typed outcome of the most recent session admission, including
    /// the 80% warning that precedes any refusal.
    pub fn last_admission(&self) -> Option<Admission> {
        self.last_admission
    }

    /// Bounded local diagnostic capture under `.fluzodrive/telemetry/`.
    ///
    /// At the diagnostics threshold the record is dropped and counted;
    /// the call still succeeds, because diagnostic saturation must not
    /// block ordinary execution. Existing capture is never modified.
    /// A returned `Err` is a genuine safety or I/O problem, not quota.
    pub fn append_diagnostic(
        &mut self,
        name: &str,
        bytes: &[u8],
    ) -> Result<DiagnosticDisposition, StoreError> {
        let Some(ledger) = self.quota.as_ref() else {
            return Err(StoreError::QuotaNotConfigured);
        };
        if !is_safe_diagnostic_name(name) {
            return Err(StoreError::UnsupportedPath);
        }
        let disposition = ledger.admit_diagnostic(bytes.len() as u64)?;
        if let DiagnosticDisposition::Dropped { reason: _, status } = disposition {
            self.diagnostics.dropped_records += 1;
            self.diagnostics.dropped_bytes = self
                .diagnostics
                .dropped_bytes
                .saturating_add(bytes.len() as u64);
            let _ = status;
            return Ok(disposition);
        }
        let directory = self.anchor_class(DIAGNOSTICS_DIRECTORY, true)?;
        let path = Self::anchored_path(&directory, name);
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(NOFOLLOW_CLOEXEC)
                .open(&path)
                .map_err(|_| StoreError::Conflict)?
        };
        #[cfg(not(unix))]
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| StoreError::Conflict)?;
        std::io::Write::write_all(&mut file, bytes).map_err(|_| StoreError::Io)?;
        file.sync_all().map_err(|_| StoreError::Io)?;
        self.diagnostics.written_records += 1;
        self.diagnostics.written_bytes = self
            .diagnostics
            .written_bytes
            .saturating_add(bytes.len() as u64);
        Ok(disposition)
    }

    pub fn diagnostic_counters(&self) -> DiagnosticCounters {
        self.diagnostics
    }

    /// The effective quota limits for this store, or `None` when opened
    /// without a guard.
    pub fn quota_limits(&self) -> Option<QuotaLimits> {
        self.quota.as_ref().map(QuotaLedger::limits)
    }

    /// Test hook: retargets the attached quota so thresholds are derived
    /// from the actual size of a freshly created journal rather than a
    /// hardcoded figure. Small synthetic limits, no real disk fills.
    #[cfg(test)]
    pub(crate) fn set_quota_limits_for_test(
        &mut self,
        limits: QuotaLimits,
    ) -> Result<(), StoreError> {
        let ledger = self.quota.as_mut().ok_or(StoreError::QuotaNotConfigured)?;
        ledger.set_limits(limits);
        Ok(())
    }

    pub async fn state(&mut self, id: &str) -> Result<Option<(String, String, i64)>, StoreError> {
        let row = sqlx::query("SELECT kind, payload, revision FROM state WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut self.connection)
            .await
            .map_err(map_error)?;
        Ok(row.map(|row| {
            (
                row.try_get::<String, _>("kind").unwrap_or_default(),
                row.try_get::<String, _>("payload").unwrap_or_default(),
                row.try_get::<i64, _>("revision").unwrap_or_default(),
            )
        }))
    }

    /// Journal records with a sequence strictly after `after`, oldest first.
    pub async fn events_after(
        &mut self,
        after: i64,
        limit: i64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let rows = sqlx::query(
            "SELECT seq, state_id, kind, payload FROM event WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&mut self.connection)
        .await
        .map_err(map_error)?;
        Ok(rows
            .into_iter()
            .map(|row| StoredEvent {
                seq: row.try_get("seq").unwrap_or_default(),
                state_id: row.try_get("state_id").unwrap_or_default(),
                kind: row.try_get("kind").unwrap_or_default(),
                payload: row.try_get("payload").unwrap_or_default(),
            })
            .collect())
    }

    /// Test hook: caps the database at `pages` 4KiB pages so a commit that
    /// needs more fails with SQLITE_FULL, the real full-storage error path.
    /// PRAGMA values cannot be bound parameters, so the already-numeric
    /// `pages` is inlined.
    #[cfg(test)]
    pub(crate) async fn cap_pages_for_test(&mut self, pages: u32) -> Result<(), StoreError> {
        // Audited: `pages` is an owned u32 rendered by Display, so the
        // inlined PRAGMA text cannot carry attacker-controlled bytes.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "PRAGMA max_page_count = {pages}"
        )))
        .execute(&mut self.connection)
        .await
        .map_err(map_error)?;
        Ok(())
    }

    /// Test hook: starts a `BEGIN IMMEDIATE` transaction and leaves it open
    /// (uncommitted) so other writers observe the lock.
    #[cfg(test)]
    pub(crate) async fn hold_write_lock_for_test(
        &mut self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>, StoreError> {
        self.connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(map_error)
    }

    /// Test hook: truncates the WAL tail to simulate a torn write. The
    /// connection must be closed first; recovery then replays only the last
    /// complete, checksum-valid transactions.
    #[cfg(test)]
    pub(crate) fn truncate_wal_tail_for_test(
        drive_directory: &Path,
        keep_bytes: u64,
    ) -> Result<(), StoreError> {
        let wal = drive_directory.join(format!("{DATABASE_FILE}-wal"));
        let metadata = fs::symlink_metadata(&wal).map_err(|_| StoreError::Io)?;
        let length = metadata.len();
        if length <= keep_bytes {
            return Err(StoreError::Io);
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&wal)
            .map_err(|_| StoreError::Io)?;
        file.set_len(keep_bytes).map_err(|_| StoreError::Io)?;
        drop(file);
        Ok(())
    }

    /// Test hook: forces a WAL checkpoint so committed pages move into the
    /// main database file.
    #[cfg(test)]
    pub(crate) async fn checkpoint_for_test(&mut self) -> Result<(), StoreError> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut self.connection)
            .await
            .map_err(map_error)?;
        Ok(())
    }

    /// Opens the drive directory anchored from `/`. When `create` is false
    /// the drive must already exist: a missing drive returns `Inaccessible`
    /// instead of being created. The purge uses `create = false` so a
    /// dry-run never creates `.fluzodrive` or any database file.
    pub(crate) fn anchor_drive_with(
        drive_directory: &Path,
        create: bool,
    ) -> Result<File, StoreError> {
        let parent = drive_directory
            .parent()
            .ok_or(StoreError::UnsupportedPath)?;
        let mut directory = File::open("/").map_err(|_| StoreError::Inaccessible)?;
        for component in parent.components() {
            if let Component::Normal(name) = component {
                #[cfg(target_os = "linux")]
                {
                    use std::os::fd::AsRawFd;
                    use std::os::unix::fs::OpenOptionsExt;
                    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
                        .join(name);
                    directory = OpenOptions::new()
                        .read(true)
                        .custom_flags(ANCHOR_FLAGS)
                        .open(path)
                        .map_err(|_| StoreError::UnsupportedPath)?;
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
            let anchored = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
                .join(DRIVE_DIRECTORY);
            match fs::symlink_metadata(&anchored) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(StoreError::Conflict);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !create {
                        return Err(StoreError::Inaccessible);
                    }
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&anchored)
                        .map_err(|error| {
                            if error.kind() == std::io::ErrorKind::NotFound {
                                StoreError::Inaccessible
                            } else {
                                StoreError::Conflict
                            }
                        })?;
                }
                Err(_) => {
                    return Err(StoreError::Inaccessible);
                }
            }
            let handle = OpenOptions::new()
                .read(true)
                .custom_flags(ANCHOR_FLAGS)
                .open(&anchored)
                .map_err(|_| StoreError::Conflict)?;
            let current =
                fs::symlink_metadata(drive_directory).map_err(|_| StoreError::Conflict)?;
            let held = handle.metadata().map_err(|_| StoreError::Conflict)?;
            if current.dev() != held.dev() || current.ino() != held.ino() {
                return Err(StoreError::Conflict);
            }
            Ok(handle)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = drive_directory;
            Err(StoreError::UnsupportedPath)
        }
    }

    fn verify_database_file(path: &Path) -> Result<(), StoreError> {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return Ok(());
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(StoreError::Conflict);
            }
        }
        #[cfg(not(unix))]
        {
            if !metadata.is_file() {
                return Err(StoreError::Conflict);
            }
        }
        Ok(())
    }

    pub(crate) fn verify_drive(&self) -> Result<(), StoreError> {
        let current =
            fs::symlink_metadata(&self.drive_directory).map_err(|_| StoreError::Conflict)?;
        let held = self
            .drive_handle
            .metadata()
            .map_err(|_| StoreError::Inaccessible)?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::MetadataExt;
            if !current.is_dir() || current.dev() != held.dev() || current.ino() != held.ino() {
                return Err(StoreError::Conflict);
            }
        }
        Ok(())
    }

    /// Opens (creating when absent) one managed class directory under the
    /// drive root with the same fd-anchored, symlink-rejecting discipline
    /// used for the drive itself. Shared by the artifact tree and the
    /// local diagnostic archive so containment is implemented once.
    #[cfg(target_os = "linux")]
    pub(crate) fn anchor_class(&self, class_dir: &str, create: bool) -> Result<File, StoreError> {
        anchor_class_in(&self.drive_handle, &self.drive_directory, class_dir, create)
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn anchor_class(&self, class_dir: &str, _create: bool) -> Result<File, StoreError> {
        let _ = class_dir;
        Err(StoreError::UnsupportedPath)
    }

    async fn verify_pragmas(connection: &mut sqlx::SqliteConnection) -> Result<(), StoreError> {
        let journal = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&mut *connection)
            .await
            .map_err(map_error)?;
        let journal: String = journal
            .try_get(0)
            .map_err(|_| StoreError::PragmaNotEnforced)?;
        if !journal.eq_ignore_ascii_case("wal") {
            return Err(StoreError::PragmaNotEnforced);
        }
        let synchronous = sqlx::query("PRAGMA synchronous")
            .fetch_one(&mut *connection)
            .await
            .map_err(map_error)?;
        let synchronous: i64 = synchronous
            .try_get(0)
            .map_err(|_| StoreError::PragmaNotEnforced)?;
        if synchronous != 2 {
            return Err(StoreError::PragmaNotEnforced);
        }
        let foreign_keys = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&mut *connection)
            .await
            .map_err(map_error)?;
        let foreign_keys: i64 = foreign_keys
            .try_get(0)
            .map_err(|_| StoreError::PragmaNotEnforced)?;
        if foreign_keys != 1 {
            return Err(StoreError::PragmaNotEnforced);
        }
        Ok(())
    }

    /// Creates the baseline schema at the current target version. A brand-new
    /// store is built directly at `TARGET_VERSION` so it never needs a
    /// migration; stores written by an earlier build reach the same shape by
    /// the explicit steps in `crate::migrations`.
    async fn create_baseline(connection: &mut sqlx::SqliteConnection) -> Result<(), StoreError> {
        let mut transaction = connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(map_error)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS state (
                 id TEXT PRIMARY KEY,
                 kind TEXT NOT NULL,
                 payload TEXT NOT NULL,
                 revision INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL DEFAULT 0
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        sqlx::query("CREATE INDEX IF NOT EXISTS state_kind_idx ON state(kind)")
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS event (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 state_id TEXT NOT NULL REFERENCES state(id),
                 kind TEXT NOT NULL,
                 payload TEXT NOT NULL
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS artifact (
                 id TEXT PRIMARY KEY,
                 state_id TEXT NOT NULL REFERENCES state(id),
                 kind TEXT NOT NULL,
                 provenance TEXT NOT NULL,
                 stored_bytes INTEGER NOT NULL,
                 content_hash TEXT NOT NULL,
                 capture_flags TEXT NOT NULL
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        // OBS-01 required SQLite audit path. Audit rows are committed in
        // the same durable unit as the work they authorize or record and
        // are never produced by the diagnostic sinks.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS audit (
                 id TEXT PRIMARY KEY,
                 category TEXT NOT NULL,
                 subject TEXT NOT NULL,
                 action TEXT NOT NULL,
                 outcome TEXT NOT NULL,
                 correlation_id TEXT NOT NULL,
                 detail TEXT NOT NULL,
                 created_ms INTEGER NOT NULL
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        // DAT-02d active/attached session protection. A session with a
        // row here is being used and purge must refuse it by name. The
        // marker is durable rather than in-memory because the process that
        // attaches a session is not the process that later purges it.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS session_attachment (
                 state_id TEXT PRIMARY KEY REFERENCES state(id),
                 attached_at INTEGER NOT NULL
             )",
        )
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        sqlx::query("INSERT OR IGNORE INTO store_meta (key, value) VALUES ('schema_version', ?1)")
            .bind(SCHEMA_VERSION)
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        transaction.commit().await.map_err(map_error)
    }
}

/// Test-only capture of the last erased driver error at the connect
/// boundary, so a fixture that sees `Inaccessible` can print what the
/// driver actually reported instead of guessing.
#[cfg(test)]
pub(crate) static LAST_CONNECT_ERROR: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
fn record_connect_error(error: &sqlx::Error) {
    if let Ok(mut slot) = LAST_CONNECT_ERROR.lock() {
        *slot = Some(error.to_string());
    }
}

#[cfg(test)]
pub(crate) fn last_connect_error() -> String {
    LAST_CONNECT_ERROR
        .lock()
        .map(|slot| {
            slot.clone()
                .unwrap_or_else(|| "none recorded by this process".to_owned())
        })
        .unwrap_or_else(|_| "diagnostic lock poisoned".to_owned())
}

/// Opens (creating when absent) one managed class directory under an
/// already-anchored drive handle with the same fd-anchored,
/// symlink-rejecting discipline used for the drive itself. Extracted from
/// `Store::anchor_class` so the telemetry purge can anchor the diagnostic
/// directory from its own read-only drive handle without opening the
/// database.
#[cfg(target_os = "linux")]
pub(crate) fn anchor_class_in(
    drive_handle: &File,
    drive_directory: &Path,
    class_dir: &str,
    create: bool,
) -> Result<File, StoreError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    let base = PathBuf::from(format!("/proc/self/fd/{}", drive_handle.as_raw_fd())).join(class_dir);
    match fs::symlink_metadata(&base) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StoreError::Conflict);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Err(StoreError::Inaccessible);
            }
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&base)
                .map_err(|_| StoreError::Conflict)?;
        }
        Err(_) => return Err(StoreError::Inaccessible),
    }
    let handle = OpenOptions::new()
        .read(true)
        .custom_flags(ANCHOR_FLAGS)
        .open(&base)
        .map_err(|_| StoreError::Conflict)?;
    let current =
        fs::symlink_metadata(drive_directory.join(class_dir)).map_err(|_| StoreError::Conflict)?;
    let held = handle.metadata().map_err(|_| StoreError::Conflict)?;
    if current.dev() != held.dev() || current.ino() != held.ino() {
        return Err(StoreError::Conflict);
    }
    Ok(handle)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn anchor_class_in(
    _drive_handle: &File,
    _drive_directory: &Path,
    _class_dir: &str,
    _create: bool,
) -> Result<File, StoreError> {
    Err(StoreError::UnsupportedPath)
}

pub(crate) fn map_error(error: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(database) = &error {
        // SQLite reports extended result codes (e.g. SQLITE_CONSTRAINT_FOREIGNKEY
        // = 781); the primary class is the low byte.
        let primary = database
            .code()
            .as_deref()
            .and_then(|code| code.parse::<i32>().ok())
            .map(|code| code & 0xFF);
        match primary {
            Some(5) | Some(6) => return StoreError::Busy,
            Some(11) => return StoreError::Corrupt,
            Some(13) => return StoreError::Full,
            Some(19) => return StoreError::Constraint,
            _ => {}
        }
    }
    StoreError::Io
}
