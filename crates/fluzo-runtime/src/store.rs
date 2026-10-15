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

use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, Row};

pub const DRIVE_DIRECTORY: &str = ".fluzodrive";
pub const DATABASE_FILE: &str = "state.sqlite3";
pub(crate) const SCHEMA_VERSION: &str = "2";

// O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC, matching the configuration writer.
pub(crate) const ANCHOR_FLAGS: i32 = 0x20000 | 0x10000 | 0x800;

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
}

impl Store {
    /// Opens (creating when absent) the workspace journal under a protected
    /// `.fluzodrive` directory. Fails closed before any write when the
    /// directory, the database file or the parent identity is unsafe.
    pub async fn open(workspace_root: &Path) -> Result<Self, StoreError> {
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
        let drive_directory = workspace_root.join(DRIVE_DIRECTORY);
        let drive_handle = Self::anchor_drive(&drive_directory)?;
        let database_path = drive_directory.join(DATABASE_FILE);
        Self::verify_database_file(&database_path)?;

        let options = SqliteConnectOptions::new()
            .filename(&database_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::ZERO);
        let mut connection = sqlx::SqliteConnection::connect_with(&options)
            .await
            .map_err(|_| StoreError::Inaccessible)?;
        Self::verify_pragmas(&mut connection).await?;
        Self::migrate(&mut connection).await?;
        Ok(Self {
            connection,
            drive_directory,
            drive_handle,
        })
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
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(map_error)?;
        for update in updates {
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

    fn anchor_drive(drive_directory: &Path) -> Result<File, StoreError> {
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

    async fn migrate(connection: &mut sqlx::SqliteConnection) -> Result<(), StoreError> {
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
                 revision INTEGER NOT NULL
             )",
        )
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
        sqlx::query("INSERT OR IGNORE INTO store_meta (key, value) VALUES ('schema_version', ?1)")
            .bind(SCHEMA_VERSION)
            .execute(&mut *transaction)
            .await
            .map_err(map_error)?;
        // Bootstrap-stage marker update for stores created before the artifact
        // table existed. The versioned migration framework is DAT-02c (#74).
        sqlx::query(
            "UPDATE store_meta SET value = ?1 WHERE key = 'schema_version' AND value <> ?1",
        )
        .bind(SCHEMA_VERSION)
        .execute(&mut *transaction)
        .await
        .map_err(map_error)?;
        transaction.commit().await.map_err(map_error)
    }
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
