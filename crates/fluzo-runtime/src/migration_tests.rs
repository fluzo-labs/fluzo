//! DAT-02c focused tests: explicit versioned migrations, WAL-inclusive
//! backups, fail-closed reopen and explicit restore.
//!
//! A "v1 store" is built here with the raw DAT-01 baseline DDL rather than
//! through `Store`, so the migration path is exercised against a database
//! that genuinely has the previous shape.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sqlx::sqlite::SqliteConnection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, Row};

use crate::migrations::{
    self, BACKUP_DIRECTORY, BASELINE_VERSION, MIGRATIONS, MigrationPolicy, MigrationState,
    TARGET_VERSION,
};
use crate::store::{DATABASE_FILE, DRIVE_DIRECTORY, Store, StoreError};

const CHILD_MARKER: &str = "__fluzo_migration_child__";
const CHILD_CRASH: &str = "__fluzo_migration_crash__";
static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Root {
    path: PathBuf,
}

impl Root {
    fn new(label: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        // Canonicalize the temp base so a symlinked TMPDIR cannot make
        // the fixture root's identity differ from what the store sees,
        // and force 0700 explicitly so the runner's umask cannot widen
        // or narrow the fixture's permissions.
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-dat02c-{label}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("root mode");
        }
        Self { path }
    }

    fn drive(&self) -> PathBuf {
        self.path.join(DRIVE_DIRECTORY)
    }

    fn database(&self) -> PathBuf {
        self.drive().join(DATABASE_FILE)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Builds a database carrying the DAT-01 baseline schema and one committed
/// unit, exactly as a store written before DAT-02c would look.
async fn create_v1_store(path: &Path) {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("create v1 store");
    for statement in [
        "CREATE TABLE store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        "CREATE TABLE state (id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL, \
         revision INTEGER NOT NULL)",
        "CREATE TABLE event (seq INTEGER PRIMARY KEY AUTOINCREMENT, state_id TEXT NOT NULL \
         REFERENCES state(id), kind TEXT NOT NULL, payload TEXT NOT NULL)",
        "INSERT INTO store_meta (key, value) VALUES ('schema_version', '1')",
        "INSERT INTO state (id, kind, payload, revision) VALUES ('s1', 'task', 'before-migration', 1)",
        "INSERT INTO event (state_id, kind, payload) VALUES ('s1', 'created', 'unit-one')",
    ] {
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&mut connection)
            .await
            .expect("v1 baseline DDL");
    }
    connection.close().await.expect("close v1 store");
}

async fn state_columns(path: &Path) -> Vec<String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open read-only");
    let rows = sqlx::query("SELECT name FROM pragma_table_info('state') ORDER BY cid")
        .fetch_all(&mut connection)
        .await
        .expect("table info");
    let columns: Vec<String> = rows
        .into_iter()
        .map(|row| row.try_get::<String, _>("name").expect("column name"))
        .collect();
    connection.close().await.expect("close");
    columns
}

async fn indexed(path: &Path, index: &str) -> bool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open read-only");
    let row = sqlx::query("SELECT count(*) AS n FROM sqlite_master WHERE type='index' AND name=?1")
        .bind(index)
        .fetch_one(&mut connection)
        .await
        .expect("index lookup");
    let found = row.try_get::<i64, _>("n").expect("count") > 0;
    connection.close().await.expect("close");
    found
}

/// Runs raw statements against a database outside the `Store` contract, used
/// to build pre-migration fixtures and to inject deterministic failures.
async fn raw_execute(path: &Path, statement: &str) {
    let options = SqliteConnectOptions::new().filename(path);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open for raw statement");
    sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
        .execute(&mut connection)
        .await
        .expect("raw statement");
    connection.close().await.expect("close");
}

async fn meta_value(path: &Path, key: &str) -> Option<String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open read-only");
    let row = sqlx::query("SELECT value FROM store_meta WHERE key = ?1")
        .bind(key)
        .fetch_optional(&mut connection)
        .await
        .expect("meta read");
    let value = row.map(|row| row.try_get::<String, _>("value").expect("meta value"));
    connection.close().await.expect("close");
    value
}

async fn table_exists(path: &Path, name: &str) -> bool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open read-only");
    let row = sqlx::query("SELECT count(*) AS n FROM sqlite_master WHERE type='table' AND name=?1")
        .bind(name)
        .fetch_one(&mut connection)
        .await
        .expect("table lookup");
    let found = row.try_get::<i64, _>("n").expect("count") > 0;
    connection.close().await.expect("close");
    found
}

async fn row_payload(path: &Path, id: &str) -> Option<String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .foreign_keys(false);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("open read-only");
    let row = sqlx::query("SELECT payload FROM state WHERE id = ?1")
        .bind(id)
        .fetch_optional(&mut connection)
        .await
        .expect("state read");
    let payload = row.map(|row| row.try_get::<String, _>("payload").expect("payload"));
    connection.close().await.expect("close");
    payload
}

fn payload_in_main_file(path: &Path, payload: &str) -> bool {
    fs::read(path)
        .map(|bytes| {
            bytes
                .windows(payload.len())
                .any(|window| window == payload.as_bytes())
        })
        .unwrap_or(false)
}

fn backup_for(root: &Root, from: u32, to: u32) -> Option<PathBuf> {
    let backups = root.drive().join(BACKUP_DIRECTORY);
    let entries = fs::read_dir(backups).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&format!("state-v{from}-to-v{to}-")) && name.ends_with(".sqlite3") {
            return Some(entry.path());
        }
    }
    None
}

async fn open(root: &Root, policy: MigrationPolicy) -> Result<Store, StoreError> {
    Store::open_with_policy(&root.path, policy).await
}

/// Only these classes are treated as possible environment flakes worth a
/// bounded retry. Semantic refusals (MigrationFailed, SchemaTooNew, ...)
/// are never retried: they are the contract talking.
fn is_env_flake(error: StoreError) -> bool {
    matches!(
        error,
        StoreError::Inaccessible | StoreError::Io | StoreError::Busy
    )
}

/// Opens with bounded backoff on environment-flake errors only. Every
/// failed attempt is logged with a full environment report before the
/// retry, so a recovery is still visible evidence, not a hidden flake.
async fn open_logged(
    root: &Root,
    policy: MigrationPolicy,
    context: &str,
) -> Result<Store, StoreError> {
    let mut delay = Duration::from_millis(50);
    let max_attempts = 4;
    for attempt in 1..=max_attempts {
        match open(root, policy).await {
            Ok(store) => {
                if attempt > 1 {
                    crate::test_diagnostics::note_recovered(&root.path, context, attempt);
                }
                return Ok(store);
            }
            Err(error) if attempt < max_attempts && is_env_flake(error) => {
                crate::test_diagnostics::report_and_log(
                    &root.path,
                    &format!("{context} (attempt {attempt}, retrying)"),
                    error,
                );
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("loop returns on every attempt");
}

async fn expect_open(root: &Root, policy: MigrationPolicy, context: &str) -> Store {
    match open_logged(root, policy, context).await {
        Ok(store) => store,
        Err(error) => {
            crate::test_diagnostics::report_and_log(
                &root.path,
                &format!("{context}: expected open to succeed"),
                error,
            );
            panic!("{context}: open failed with {error:?}");
        }
    }
}

async fn expect_open_refused(
    root: &Root,
    policy: MigrationPolicy,
    expected: StoreError,
    context: &str,
) {
    match open_logged(root, policy, context).await {
        Ok(_) => panic!("{context}: open unexpectedly succeeded, expected {expected:?}"),
        Err(error) if error == expected => {}
        Err(error) => {
            crate::test_diagnostics::report_and_log(
                &root.path,
                &format!("{context}: expected refusal {expected:?}"),
                error,
            );
            panic!("{context}: expected {expected:?}, got {error:?}");
        }
    }
}

async fn expect_restore(root: &Root, backup: &Path, context: &str) -> Store {
    match Store::restore(&root.path, backup).await {
        Ok(store) => store,
        Err(error) => {
            crate::test_diagnostics::report_and_log(
                &root.path,
                &format!("{context}: explicit restore failed"),
                error,
            );
            panic!("{context}: restore failed with {error:?}");
        }
    }
}

// --- registry -------------------------------------------------------------

#[test]
fn the_migration_registry_is_contiguous_and_forward_only() {
    assert_eq!(
        MIGRATIONS.first().map(|step| step.from_version()),
        Some(BASELINE_VERSION),
        "the registry must start at the baseline schema"
    );
    assert_eq!(
        MIGRATIONS.last().map(|step| step.to_version()),
        Some(TARGET_VERSION),
        "the registry must end at the supported target"
    );
    for pair in MIGRATIONS.windows(2) {
        assert_eq!(
            pair[0].to_version(),
            pair[1].from_version(),
            "no gap is allowed between consecutive steps"
        );
    }
    for step in MIGRATIONS {
        assert!(
            step.to_version() > step.from_version(),
            "down-migrations must not exist"
        );
        assert!(
            step.to_version() != BASELINE_VERSION,
            "no step may migrate back to the baseline"
        );
    }
}

#[tokio::test]
async fn a_fresh_store_is_at_the_target_version_with_no_migration_run() {
    let root = Root::new("fresh");
    let mut store = expect_open(&root, MigrationPolicy::Migrate, "fresh open").await;
    assert_eq!(store.schema_version().await.unwrap(), TARGET_VERSION);
    assert!(
        store.migration_log().await.unwrap().is_empty(),
        "a brand-new store must not record a migration"
    );
    assert!(
        state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned()),
        "the fresh shape must already match the migrated shape"
    );
}

// --- policy behaviour -----------------------------------------------------

#[tokio::test]
async fn read_only_policy_refuses_an_older_store_without_touching_it() {
    let root = Root::new("readonly");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;

    expect_open_refused(
        &root,
        MigrationPolicy::ReadOnly,
        StoreError::MigrationRequired,
        "read-only refusal",
    )
    .await;
    assert_eq!(
        meta_value(&root.database(), "schema_version")
            .await
            .as_deref(),
        Some("1"),
        "a refused read-only open must not touch the version marker"
    );
    assert!(
        !state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned()),
        "a refused read-only open must not migrate"
    );
    assert!(
        !table_exists(&root.database(), "migration_run").await,
        "a refused read-only open must not even create the audit table"
    );
    assert!(
        backup_for(&root, 1, 2).is_none(),
        "a read-only open must not take a migration backup"
    );
}

#[tokio::test]
async fn preserve_policy_opens_an_older_store_without_migrating() {
    let root = Root::new("preserve");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;

    let mut store = expect_open(
        &root,
        MigrationPolicy::Preserve,
        "preserve opens older store",
    )
    .await;
    assert_eq!(store.schema_version().await.unwrap(), BASELINE_VERSION);
    assert!(
        !state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned()),
        "preserve must leave the schema alone"
    );
}

#[tokio::test]
async fn a_newer_schema_version_is_refused_by_every_policy_and_left_untouched() {
    for policy in [
        MigrationPolicy::ReadOnly,
        MigrationPolicy::Migrate,
        MigrationPolicy::Preserve,
    ] {
        let root = Root::new("too-new");
        fs::create_dir_all(root.drive()).unwrap();
        create_v1_store(&root.database()).await;
        raw_execute(
            &root.database(),
            "UPDATE store_meta SET value = '99' WHERE key = 'schema_version'",
        )
        .await;
        expect_open_refused(
            &root,
            policy,
            StoreError::SchemaTooNew,
            &format!("{policy:?} refuses schema above target"),
        )
        .await;
        assert_eq!(
            meta_value(&root.database(), "schema_version")
                .await
                .as_deref(),
            Some("99"),
            "a refused open must not rewrite the version marker"
        );
        assert_eq!(
            row_payload(&root.database(), "s1").await.as_deref(),
            Some("before-migration"),
            "a refused open must not touch stored data"
        );
        assert!(
            !table_exists(&root.database(), "migration_run").await,
            "{policy:?} must not write an audit table before the version check"
        );
    }
}

// --- backup ---------------------------------------------------------------

#[tokio::test]
async fn migrate_backs_up_verifies_and_applies_the_registered_step() {
    let root = Root::new("migrate");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;

    let mut store = expect_open(&root, MigrationPolicy::Migrate, "initial migrate").await;
    assert_eq!(store.schema_version().await.unwrap(), TARGET_VERSION);
    assert!(
        state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned())
    );
    assert!(indexed(&root.database(), "state_kind_idx").await);

    let log = store.migration_log().await.unwrap();
    assert_eq!(
        log.len(),
        MIGRATIONS.len(),
        "every registered step must be recorded"
    );
    for (index, step) in MIGRATIONS.iter().enumerate() {
        assert_eq!(log[index].from_version, step.from_version());
        assert_eq!(log[index].to_version, step.to_version());
        assert_eq!(log[index].state, MigrationState::Succeeded);
    }
    let backup = PathBuf::from(log[0].backup_path.as_ref().expect("backup recorded"));
    assert!(backup.exists());
    assert_eq!(
        backup.parent().unwrap().file_name().unwrap(),
        BACKUP_DIRECTORY,
        "backups must live inside the protected drive area"
    );
    assert!(
        table_exists(&root.database(), "artifact").await,
        "the v1 -> v2 step must have added the artifact table"
    );

    // The pre-migration data survived the step.
    let (kind, payload, revision) = store.state("s1").await.unwrap().unwrap();
    assert_eq!(
        (kind.as_str(), payload.as_str(), revision),
        ("task", "before-migration", 1)
    );

    // Reopening is a no-op: the store is already at the target.
    drop(store);
    let mut again = expect_open(&root, MigrationPolicy::Migrate, "reopen after migration").await;
    assert_eq!(again.schema_version().await.unwrap(), TARGET_VERSION);
    assert_eq!(again.migration_log().await.unwrap().len(), MIGRATIONS.len());
}

#[tokio::test]
async fn the_backup_contains_committed_rows_that_are_only_in_the_wal() {
    let root = Root::new("wal");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;

    // Open without migrating, then commit a unit. With `synchronous=FULL`
    // and a tiny WAL the commit lives in `state.sqlite3-wal`, not in the
    // main database file.
    let mut store = expect_open(
        &root,
        MigrationPolicy::Preserve,
        "preserve before wal backup",
    )
    .await;
    store
        .commit_unit(
            &[crate::store::StateUpdate {
                id: "wal-only".to_owned(),
                kind: "task".to_owned(),
                payload: "only-in-the-wal".to_owned(),
            }],
            &[crate::store::EventRecord {
                state_id: "wal-only".to_owned(),
                kind: "appended".to_owned(),
                payload: "wal-payload".to_owned(),
            }],
        )
        .await
        .expect("commit");

    assert!(
        !payload_in_main_file(&root.database(), "only-in-the-wal"),
        "precondition: the row must not be in the main file yet"
    );

    let backup = migrations::take_backup(store.connection_for_test(), &root.drive(), 1, 2)
        .await
        .expect("backup");
    assert!(
        payload_in_main_file(&backup, "only-in-the-wal"),
        "the backup must include committed pages still sitting in the WAL"
    );
    migrations::verify_backup(&backup, 1, MIGRATIONS[0])
        .await
        .expect("backup verifies as a pre-migration snapshot");
}

#[tokio::test]
async fn backup_directory_and_files_are_owner_only() {
    let root = Root::new("perms");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;
    let mut store = expect_open(&root, MigrationPolicy::Migrate, "migrate for perms").await;
    let backup = backup_for(&root, 1, 2).expect("backup exists");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = fs::metadata(root.drive().join(BACKUP_DIRECTORY))
            .unwrap()
            .permissions()
            .mode();
        let file_mode = fs::metadata(&backup).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(file_mode & 0o777, 0o600);
    }
    let _ = &mut store;
}

// --- failure and reopen ---------------------------------------------------

#[tokio::test]
async fn a_failed_step_rolls_back_the_schema_and_is_recorded_as_failed() {
    let root = Root::new("failed");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;
    // Pre-create the index the step wants to add so the step fails on a
    // duplicate object name. Deterministic, no monkey-patching.
    raw_execute(
        &root.database(),
        "CREATE INDEX state_kind_idx ON state(kind)",
    )
    .await;

    expect_open_refused(
        &root,
        MigrationPolicy::Migrate,
        StoreError::MigrationFailed,
        "open with pre-existing index",
    )
    .await;
    assert_eq!(
        state_columns(&root.database()).await,
        vec!["id", "kind", "payload", "revision"],
        "the whole step must roll back, not half-apply"
    );
    // The earlier registered step committed before this one was reached, so
    // the store sits at the version that step produced and no further.
    assert_eq!(
        meta_value(&root.database(), "schema_version")
            .await
            .as_deref(),
        Some("2"),
        "a failed step must not advance the version marker"
    );

    // The backup taken for the failed step is of the untouched pre-step
    // schema and still verifies as a restore point.
    let backup = backup_for(&root, 2, 3).expect("backup exists");
    migrations::verify_backup(&backup, 2, MIGRATIONS[1])
        .await
        .expect("the backup of the untouched schema still verifies");
}

#[tokio::test]
async fn a_failed_migration_is_refused_on_reopen_and_nothing_recovers_automatically() {
    let root = Root::new("refuse");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;
    raw_execute(
        &root.database(),
        "CREATE INDEX state_kind_idx ON state(kind)",
    )
    .await;
    expect_open_refused(
        &root,
        MigrationPolicy::Migrate,
        StoreError::MigrationFailed,
        "first migrate with pre-existing index",
    )
    .await;
    // Every policy refuses: a broken step may not be ridden over by a
    // read-only inspection either.
    for policy in [
        MigrationPolicy::Migrate,
        MigrationPolicy::ReadOnly,
        MigrationPolicy::Preserve,
    ] {
        expect_open_refused(
            &root,
            policy,
            StoreError::MigrationFailed,
            &format!("{policy:?} reopen after failed migration"),
        )
        .await;
    }
    assert_eq!(
        meta_value(&root.database(), "schema_version")
            .await
            .as_deref(),
        Some("2"),
        "only the step that actually committed may advance the live schema"
    );
    assert!(
        table_exists(&root.database(), "artifact").await,
        "the committed v1 -> v2 step must be present"
    );
    assert_eq!(
        row_payload(&root.database(), "s1").await.as_deref(),
        Some("before-migration"),
        "nothing may rewrite stored data by itself"
    );
    assert!(
        !state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned()),
        "the live schema must still be pre-migration"
    );
}

#[tokio::test]
async fn an_interrupted_migration_is_refused_on_reopen() {
    let root = Root::new("interrupted");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;

    let mut child = spawn_child(CHILD_CRASH, &root.path);
    let status = child.wait().expect("child exits");
    assert!(
        !status.success(),
        "the fixture must die on the crash point, not finish; status={status:?}"
    );
    assert!(
        crash_marker(&root),
        "the fixture must reach the durable running record"
    );

    // The audit row is durable and non-terminal, so reopen refuses.
    expect_open_refused(
        &root,
        MigrationPolicy::Migrate,
        StoreError::MigrationInterrupted,
        "reopen after crash",
    )
    .await;
    assert!(
        !state_columns(&root.database())
            .await
            .contains(&"updated_at".to_owned()),
        "the schema must not have advanced past the crash"
    );
    assert!(
        backup_for(&root, 1, 2).is_some(),
        "the pre-migration backup must survive for recovery"
    );
}

// --- restore --------------------------------------------------------------

#[tokio::test]
async fn restore_returns_the_exact_pre_migration_state() {
    let root = Root::new("restore");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;
    // Interrupt a migration the way it really happens in the field: the
    // process dies on the durable `running` audit record.
    let mut child = spawn_child(CHILD_CRASH, &root.path);
    child.wait().expect("child exits");
    assert!(
        crash_marker(&root),
        "the fixture must reach the durable running record"
    );
    expect_open_refused(
        &root,
        MigrationPolicy::Migrate,
        StoreError::MigrationInterrupted,
        "pre-restore refusal",
    )
    .await;
    let backup = backup_for(&root, 1, 2).expect("backup exists");

    let mut store = expect_restore(&root, &backup, "post-interrupt restore").await;
    assert_eq!(
        store.schema_version().await.unwrap(),
        BASELINE_VERSION,
        "restore must land on the pre-migration version"
    );
    assert_eq!(
        state_columns(&root.database()).await,
        vec!["id", "kind", "payload", "revision"],
        "the restored schema must be exactly the pre-migration shape"
    );
    let (kind, payload, revision) = store.state("s1").await.unwrap().unwrap();
    assert_eq!(
        (kind.as_str(), payload.as_str(), revision),
        ("task", "before-migration", 1)
    );

    let log = store.migration_log().await.unwrap();
    assert_eq!(
        log.last().map(|record| record.state),
        Some(MigrationState::Restored)
    );
    assert_eq!(
        log.last().and_then(|record| record.backup_path.as_deref()),
        Some(backup.to_string_lossy().as_ref())
    );

    // After a restore the store is usable again and can be migrated.
    drop(store);
    let mut migrated =
        expect_open(&root, MigrationPolicy::Migrate, "re-migrate after restore").await;
    assert_eq!(migrated.schema_version().await.unwrap(), TARGET_VERSION);
}

#[tokio::test]
async fn restore_rejects_paths_outside_the_backup_directory() {
    let root = Root::new("restore-path");
    fs::create_dir_all(root.drive().join(BACKUP_DIRECTORY)).unwrap();
    create_v1_store(&root.database()).await;
    let outside = root.path.join("sneaky.sqlite3");
    fs::copy(root.database(), &outside).unwrap();

    assert_eq!(
        Store::restore(&root.path, &outside).await.err(),
        Some(StoreError::UnsupportedPath)
    );
    let elsewhere = root.drive().join("elsewhere.sqlite3");
    fs::copy(root.database(), &elsewhere).unwrap();
    assert_eq!(
        Store::restore(&root.path, &elsewhere).await.err(),
        Some(StoreError::UnsupportedPath),
        "the backup must live in the drive's own backups directory"
    );
}

#[tokio::test]
async fn restore_rejects_symlinked_and_hardlinked_backups() {
    let root = Root::new("restore-link");
    let backups = root.drive().join(BACKUP_DIRECTORY);
    fs::create_dir_all(&backups).unwrap();
    create_v1_store(&root.database()).await;
    let real = root.path.join("real-backup.sqlite3");
    fs::copy(root.database(), &real).unwrap();

    let symlink = backups.join("state-v1-to-v2-1.sqlite3");
    std::os::unix::fs::symlink(&real, &symlink).unwrap();
    assert_eq!(
        Store::restore(&root.path, &symlink).await.err(),
        Some(StoreError::Conflict)
    );

    fs::remove_file(&symlink).unwrap();
    fs::copy(&real, &symlink).unwrap();
    let hardlink = backups.join("state-v1-to-v2-2.sqlite3");
    fs::hard_link(&symlink, &hardlink).unwrap();
    assert_eq!(
        Store::restore(&root.path, &hardlink).await.err(),
        Some(StoreError::Conflict)
    );
}

#[tokio::test]
async fn restore_refuses_while_another_writer_holds_the_store() {
    let root = Root::new("restore-busy");
    fs::create_dir_all(root.drive()).unwrap();
    create_v1_store(&root.database()).await;
    let mut store = expect_open(&root, MigrationPolicy::Preserve, "open for holding").await;
    let holder = store
        .connection_for_test()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("write lock");

    let backup = {
        let backups = root.drive().join(BACKUP_DIRECTORY);
        fs::create_dir_all(&backups).unwrap();
        let candidate = backups.join("state-v1-to-v2-1.sqlite3");
        fs::copy(root.database(), &candidate).unwrap();
        candidate
    };

    assert_eq!(
        Store::restore(&root.path, &backup).await.err(),
        Some(StoreError::Busy),
        "restore must not replace a store another writer holds"
    );
    holder.rollback().await.expect("release");
    drop(store);
    let _restored = expect_restore(&root, &backup, "restore after holder release").await;
}

#[tokio::test]
async fn restore_rejects_a_corrupt_backup() {
    let root = Root::new("restore-corrupt");
    let backups = root.drive().join(BACKUP_DIRECTORY);
    fs::create_dir_all(&backups).unwrap();
    create_v1_store(&root.database()).await;
    let candidate = backups.join("state-v1-to-v2-1.sqlite3");
    fs::copy(root.database(), &candidate).unwrap();
    // Truncate into something that is not a database.
    fs::write(&candidate, b"not a sqlite file at all").unwrap();

    assert_eq!(
        Store::restore(&root.path, &candidate).await.err(),
        Some(StoreError::BackupVerificationFailed)
    );
    assert!(
        fs::symlink_metadata(root.database()).is_ok(),
        "a rejected restore must leave the live store in place"
    );
}

// --- child fixtures -------------------------------------------------------

/// The file the crash fixture leaves behind. The harness captures a child's
/// streams and `abort()` discards them, so the crash point is announced
/// through the filesystem instead.
fn crash_marker(root: &Root) -> bool {
    fs::symlink_metadata(root.drive().join("crash-after-running-record")).is_ok()
}

fn spawn_child(mode: &str, root: &Path) -> Child {
    let executable = std::env::current_exe().expect("current exe");
    Command::new(executable)
        .arg("--exact")
        .arg("migration_tests::child_mode")
        .env(CHILD_MARKER, mode)
        .env("FLUZO_TEST_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn child")
}

#[tokio::test]
async fn child_mode() {
    let mode = std::env::var(CHILD_MARKER).unwrap_or_default();
    if mode.is_empty() {
        return;
    }
    let root = PathBuf::from(std::env::var("FLUZO_TEST_ROOT").expect("test root"));
    match mode.as_str() {
        CHILD_CRASH => {
            migrations::CRASH_AFTER_RUNNING_RECORD.store(true, Ordering::SeqCst);
            let outcome = Store::open_with_policy(&root, MigrationPolicy::Migrate).await;
            panic!("child should have aborted, got {outcome:?}");
        }
        other => panic!("unknown child mode {other}"),
    }
}
