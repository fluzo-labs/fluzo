//! DAT-02c: explicit versioned migrations and restorable store backups.
//!
//! The schema is an explicit, ordered, forward-only list of steps. Every step
//! is applied by the store owner during open under an explicit
//! [`MigrationPolicy`], and every mutation of the schema is preceded by a
//! checkpoint-consistent backup taken inside the protected drive area and
//! verified by reopening it. There are no down-migrations: the backup and the
//! explicit restore operation are the recovery story.
//!
//! The backup is produced with SQLite's `VACUUM INTO`, which writes a
//! transactionally consistent snapshot that includes committed pages still
//! sitting in the write-ahead log. It is not a copy of the live files: it is
//! read through SQLite's own pager while the source is locked. The raw
//! `sqlite3_backup_*` API is not reachable here because `sqlx` keeps its
//! connection handle crate-private and this workspace forbids `unsafe`.
//!
//! The `migration_run` audit table is created on demand by every policy. It
//! records the migration history and is deliberately not itself part of the
//! versioned schema evolution, so an older store can be audited before it is
//! migrated.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row};

use crate::store::StoreError;

/// Directory inside the protected drive where pre-migration backups land.
pub const BACKUP_DIRECTORY: &str = "backups";

/// Schema shape produced by the baseline `CREATE` statements (DAT-01).
pub const BASELINE_VERSION: u32 = 1;

/// Highest schema version this build understands. A store recorded above this
/// value is never partially interpreted; opening it fails closed.
pub const TARGET_VERSION: u32 = 3;

/// How the caller wants the schema prepared during open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationPolicy {
    /// Never change the schema. A store below the target is refused with
    /// `MigrationRequired`; a store above it is refused with `SchemaTooNew`.
    ReadOnly,
    /// Apply the explicit forward steps, backing up and verifying first.
    /// A store above the target is still refused with `SchemaTooNew`.
    Migrate,
    /// Open the store at its recorded version without migrating. Used by an
    /// explicit restore so the operator sees exactly what was restored.
    /// A store above the target is still refused with `SchemaTooNew`.
    Preserve,
}

/// One explicit forward schema step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationStep {
    /// v1 -> v2: add the `artifact` table (DAT-02a).
    AddArtifactTable,
    /// v2 -> v3: add `state.updated_at` and the `state_kind_idx` index.
    AddStateUpdatedAtAndKindIndex,
}

impl MigrationStep {
    pub const fn from_version(&self) -> u32 {
        match self {
            Self::AddArtifactTable => 1,
            Self::AddStateUpdatedAtAndKindIndex => 2,
        }
    }

    pub const fn to_version(&self) -> u32 {
        match self {
            Self::AddArtifactTable => 2,
            Self::AddStateUpdatedAtAndKindIndex => 3,
        }
    }

    pub const fn label(&self) -> &'static str {
        match self {
            Self::AddArtifactTable => "add artifact table",
            Self::AddStateUpdatedAtAndKindIndex => "add state.updated_at + state_kind_idx",
        }
    }
}

/// The explicit, ordered, forward-only migration path. `MIGRATIONS[n]` must
/// migrate exactly into `MIGRATIONS[n + 1]`, starting at `BASELINE_VERSION`
/// and ending at `TARGET_VERSION`. A focused test pins this contiguity so a
/// step cannot be added without closing the gap.
pub const MIGRATIONS: &[MigrationStep] = &[
    MigrationStep::AddArtifactTable,
    MigrationStep::AddStateUpdatedAtAndKindIndex,
];

/// Terminal or in-flight state of a recorded migration attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationState {
    /// Started and durably recorded, but not finished. A crash leaves the
    /// record here, which is what makes reopen refuse.
    Running,
    Succeeded,
    Failed,
    /// An explicit operator restore of a pre-migration backup.
    Restored,
}

impl MigrationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Restored => "restored",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "restored" => Ok(Self::Restored),
            _ => Err(StoreError::Corrupt),
        }
    }
}

/// One durable migration audit row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRecord {
    pub id: i64,
    pub from_version: u32,
    pub to_version: u32,
    pub state: MigrationState,
    pub backup_path: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
}

pub(crate) fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}

/// Creates the audit table if absent. Idempotent, and safe under every policy.
pub async fn ensure_audit_table(connection: &mut sqlx::SqliteConnection) -> Result<(), StoreError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS migration_run (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             from_version INTEGER NOT NULL,
             to_version INTEGER NOT NULL,
             state TEXT NOT NULL,
             backup_path TEXT,
             error TEXT,
             started_at INTEGER NOT NULL
         )",
    )
    .execute(connection)
    .await
    .map(|_| ())
    .map_err(crate::store::map_error)
}

/// True when `table` exists in this database. Used to tell a brand-new store
/// apart from an existing one before reading any domain table, so a missing
/// table is never reported as corruption.
pub async fn has_table(
    connection: &mut sqlx::SqliteConnection,
    table: &str,
) -> Result<bool, StoreError> {
    let row =
        sqlx::query("SELECT count(*) AS n FROM sqlite_master WHERE type = 'table' AND name = ?1")
            .bind(table)
            .fetch_one(connection)
            .await
            .map_err(crate::store::map_error)?;
    let count: i64 = row.try_get("n").map_err(|_| StoreError::Corrupt)?;
    Ok(count > 0)
}

/// Reads the recorded schema version. `None` means no marker is present,
/// which is how a brand-new (empty) database is recognised.
pub async fn recorded_version(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Option<u32>, StoreError> {
    let row = sqlx::query("SELECT value FROM store_meta WHERE key = 'schema_version'")
        .fetch_optional(connection)
        .await
        .map_err(crate::store::map_error)?;
    let Some(row) = row else { return Ok(None) };
    let raw: String = row.try_get("value").map_err(|_| StoreError::Corrupt)?;
    let parsed = raw.trim().parse::<u32>().map_err(|_| StoreError::Corrupt)?;
    Ok(Some(parsed))
}

/// Appends a migration audit row in its own committed transaction so it
/// survives a crash in the step that follows.
pub async fn record_run(
    connection: &mut sqlx::SqliteConnection,
    from_version: u32,
    to_version: u32,
    state: MigrationState,
    backup_path: Option<&str>,
    error: Option<&str>,
) -> Result<i64, StoreError> {
    let row = sqlx::query(
        "INSERT INTO migration_run (from_version, to_version, state, backup_path, error, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(from_version as i64)
    .bind(to_version as i64)
    .bind(state.as_str())
    .bind(backup_path)
    .bind(error)
    .bind(epoch_seconds())
    .fetch_one(connection)
    .await
    .map_err(crate::store::map_error)?;
    row.try_get("id").map_err(|_| StoreError::Io)
}

/// Writes the terminal outcome of a recorded run.
pub async fn finish_run(
    connection: &mut sqlx::SqliteConnection,
    id: i64,
    state: MigrationState,
    error: Option<&str>,
) -> Result<(), StoreError> {
    sqlx::query("UPDATE migration_run SET state = ?2, error = ?3 WHERE id = ?1")
        .bind(id)
        .bind(state.as_str())
        .bind(error)
        .execute(connection)
        .await
        .map(|_| ())
        .map_err(crate::store::map_error)
}

/// Reads the audit trail, oldest first.
pub async fn migration_log(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Vec<MigrationRecord>, StoreError> {
    if !has_table(connection, "migration_run").await? {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT id, from_version, to_version, state, backup_path, error, started_at
         FROM migration_run ORDER BY id ASC",
    )
    .fetch_all(connection)
    .await
    .map_err(crate::store::map_error)?;
    let mut records = Vec::with_capacity(rows.len());
    for row in rows {
        let state_raw: String = row.try_get("state").map_err(|_| StoreError::Corrupt)?;
        records.push(MigrationRecord {
            id: row.try_get("id").map_err(|_| StoreError::Corrupt)?,
            from_version: u32::try_from(
                row.try_get::<i64, _>("from_version")
                    .map_err(|_| StoreError::Corrupt)?,
            )
            .map_err(|_| StoreError::Corrupt)?,
            to_version: u32::try_from(
                row.try_get::<i64, _>("to_version")
                    .map_err(|_| StoreError::Corrupt)?,
            )
            .map_err(|_| StoreError::Corrupt)?,
            state: MigrationState::parse(&state_raw)?,
            backup_path: row
                .try_get::<Option<String>, _>("backup_path")
                .map_err(|_| StoreError::Corrupt)?,
            error: row
                .try_get::<Option<String>, _>("error")
                .map_err(|_| StoreError::Corrupt)?,
            started_at: row.try_get("started_at").map_err(|_| StoreError::Corrupt)?,
        });
    }
    Ok(records)
}

/// The most recent audit row, if any.
pub async fn latest_run(
    connection: &mut sqlx::SqliteConnection,
) -> Result<Option<MigrationRecord>, StoreError> {
    Ok(migration_log(connection).await?.into_iter().next_back())
}

/// Creates (or revalidates) the backup directory inside the protected drive.
/// Rejects a symlink or a non-directory at the backup path rather than
/// writing through it, mirroring the drive-directory discipline.
pub fn ensure_backup_directory(drive_directory: &Path) -> Result<PathBuf, StoreError> {
    let backups = drive_directory.join(BACKUP_DIRECTORY);
    match fs::symlink_metadata(&backups) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(StoreError::Conflict),
        Ok(metadata) if !metadata.is_dir() => return Err(StoreError::Conflict),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&backups)
                    .map_err(|_| StoreError::Conflict)?;
            }
            #[cfg(not(unix))]
            fs::DirBuilder::new()
                .create(&backups)
                .map_err(|_| StoreError::Conflict)?;
        }
        Err(_) => return Err(StoreError::Inaccessible),
    }
    Ok(backups)
}

/// Builds a backup file name from owned integers only. The result is used in
/// a `VACUUM INTO` statement whose target cannot be a bound parameter, so
/// the name is asserted free of quoting characters before it is inlined.
fn backup_name(from: u32, to: u32, index: u32) -> String {
    format!("state-v{from}-to-v{to}-{index}.sqlite3")
}

/// Takes a checkpoint-consistent snapshot of the live database into the
/// protected backup directory and returns its path.
///
/// The snapshot includes committed pages that are still in the WAL and has
/// already been checkpointed into the target file, so the backup is
/// self-contained. Nothing in the source database is modified.
pub async fn take_backup(
    connection: &mut sqlx::SqliteConnection,
    drive_directory: &Path,
    from: u32,
    to: u32,
) -> Result<PathBuf, StoreError> {
    let backups = ensure_backup_directory(drive_directory)?;
    // Pick the first free index so repeated restore/re-migrate cycles never
    // overwrite an earlier backup.
    let mut index = 1u32;
    let target = loop {
        let candidate = backups.join(backup_name(from, to, index));
        if fs::symlink_metadata(&candidate).is_err() {
            break candidate;
        }
        index += 1;
        if index > 1_000 {
            return Err(StoreError::Conflict);
        }
    };
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(StoreError::UnsupportedPath)?;
    if name.contains('\'') || name.contains('"') {
        return Err(StoreError::UnsupportedPath);
    }
    let statement = format!(
        "VACUUM INTO '{}'",
        target.to_string_lossy().replace("''", "'")
    );
    match sqlx::raw_sql(sqlx::AssertSqlSafe(statement))
        .execute(connection)
        .await
    {
        Ok(_) => {}
        Err(error) => {
            // `VACUUM INTO` can leave a partial file behind on failure. It is
            // not a valid backup, so remove it rather than report it as one.
            let _ = fs::remove_file(&target);
            return Err(crate::store::map_error(error));
        }
    }
    // Owner-only, defence in depth behind the 0700 directory.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|_| StoreError::Io)?;
    }
    Ok(target)
}

/// Reopens a backup read-only and confirms it is a usable snapshot of the
/// expected pre-migration state: integrity, the recorded version marker and
/// the pre-migration schema shape. A backup that fails here must stop the
/// migration rather than be trusted.
pub async fn verify_backup(
    backup_path: &Path,
    expected_from: u32,
    step: MigrationStep,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(backup_path).map_err(|_| StoreError::Inaccessible)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::Conflict);
    }
    let options = SqliteConnectOptions::new()
        .filename(backup_path)
        .read_only(true)
        .foreign_keys(false);
    // The snapshot is a consolidated `delete`-journal database by design, so
    // the live-store WAL/FULL PRAGMA check deliberately does not apply here.
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;

    let check = sqlx::query("PRAGMA quick_check")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let check: String = check
        .try_get(0)
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    if check != "ok" {
        return Err(StoreError::BackupVerificationFailed);
    }

    let version = recorded_version(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    if version != Some(expected_from) {
        let _ = connection.close().await;
        return Err(StoreError::BackupVerificationFailed);
    }

    let rows = sqlx::query("SELECT name FROM pragma_table_info('state')")
        .fetch_all(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let columns: Vec<String> = rows
        .into_iter()
        .map(|row| row.try_get("name").unwrap_or_default())
        .collect();
    let tables = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let tables: Vec<String> = tables
        .into_iter()
        .map(|row| row.try_get("name").unwrap_or_default())
        .collect();
    if !step_backup_matches_pre_migration(step, &columns, &tables) {
        let _ = connection.close().await;
        return Err(StoreError::BackupVerificationFailed);
    }
    // Close explicitly: dropping a sqlx connection defers release of the
    // WAL shared-memory lock, which makes a zero-timeout reopen race.
    connection
        .close()
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)
}

/// True when the snapshot still has the pre-migration shape for `step`, i.e.
/// the column or table the step adds is not present yet. A backup that
/// already contains the post-step shape means the migration state and the
/// schema disagree, so it must not be treated as a usable restore point.
fn step_backup_matches_pre_migration(
    step: MigrationStep,
    columns: &[String],
    tables: &[String],
) -> bool {
    let has = |name: &str, set: &[String]| set.iter().any(|item| item == name);
    match step {
        MigrationStep::AddArtifactTable => !has("artifact", tables),
        MigrationStep::AddStateUpdatedAtAndKindIndex => !has("updated_at", columns),
    }
}

/// Applies one explicit schema step. The caller owns the surrounding
/// transaction and the audit record.
pub async fn apply(
    connection: &mut sqlx::SqliteConnection,
    step: MigrationStep,
) -> Result<(), StoreError> {
    match step {
        MigrationStep::AddArtifactTable => {
            sqlx::query(
                "CREATE TABLE artifact (
                     id TEXT PRIMARY KEY,
                     state_id TEXT NOT NULL REFERENCES state(id),
                     kind TEXT NOT NULL,
                     provenance TEXT NOT NULL,
                     stored_bytes INTEGER NOT NULL,
                     content_hash TEXT NOT NULL,
                     capture_flags TEXT NOT NULL
                 )",
            )
            .execute(&mut *connection)
            .await
            .map_err(crate::store::map_error)?;
            Ok(())
        }
        MigrationStep::AddStateUpdatedAtAndKindIndex => {
            sqlx::query("ALTER TABLE state ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0")
                .execute(&mut *connection)
                .await
                .map_err(crate::store::map_error)?;
            sqlx::query("CREATE INDEX state_kind_idx ON state(kind)")
                .execute(&mut *connection)
                .await
                .map_err(crate::store::map_error)?;
            Ok(())
        }
    }
}

/// The step that migrates `from` to `from + 1`, if one is registered.
pub fn step_for(from: u32) -> Option<MigrationStep> {
    MIGRATIONS
        .iter()
        .find(|step| step.from_version() == from)
        .copied()
}

/// Verifies a backup for an explicit restore and returns the schema version
/// it was taken at. Unlike the pre-migration check this does not know which
/// step was intended, so it validates integrity and the recorded marker and
/// requires the version to be one this build understands.
pub async fn verify_restore_backup(backup_path: &Path) -> Result<u32, StoreError> {
    let metadata = fs::symlink_metadata(backup_path).map_err(|_| StoreError::Inaccessible)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::Conflict);
    }
    let options = SqliteConnectOptions::new()
        .filename(backup_path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = sqlx::SqliteConnection::connect_with(&options)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let check = sqlx::query("PRAGMA quick_check")
        .fetch_one(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let check: String = check
        .try_get(0)
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    if check != "ok" {
        return Err(StoreError::BackupVerificationFailed);
    }
    let version = recorded_version(&mut connection)
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    let Some(version) = version else {
        return Err(StoreError::BackupVerificationFailed);
    };
    if version > TARGET_VERSION {
        let _ = connection.close().await;
        return Err(StoreError::SchemaTooNew);
    }
    connection
        .close()
        .await
        .map_err(|_| StoreError::BackupVerificationFailed)?;
    Ok(version)
}

/// Test-only crash point. A fixture arms it, then the migration path aborts
/// the process immediately after committing the durable `running` audit
/// record, which proves the reopen refusal is real rather than asserted.
/// Never compiled into a non-test build.
/// Name of the file the test crash point writes before aborting.
#[cfg(test)]
pub(crate) const CRASH_MARKER_FILE: &str = "crash-after-running-record";

#[cfg(test)]
pub(crate) static CRASH_AFTER_RUNNING_RECORD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
