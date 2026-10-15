//! Test-only environment diagnostics for store open/reopen failures.
//!
//! GitHub-hosted runners have shown non-deterministic
//! `StoreError::Inaccessible` from opens that pass locally. These helpers
//! capture the environment facts that distinguish an environment flake
//! from a real defect — TMPDIR, the effective umask, and the metadata of
//! the workspace root, drive directory, database file and WAL/shared-memory
//! sidecars — plus the last driver error erased at the connect boundary.
//! Every report goes to stdout AND to a fixture log file inside the test
//! root, so a failure is diagnosable from the CI log alone.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crate::store::{DATABASE_FILE, DRIVE_DIRECTORY, StoreError};

pub(crate) const FIXTURE_LOG: &str = "fixture-diagnostics.log";

fn describe(path: &Path) -> String {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let mut line = format!(
                "symlink={}, dir={}, file={}",
                metadata.file_type().is_symlink(),
                metadata.is_dir(),
                metadata.is_file(),
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                line.push_str(&format!(
                    ", mode={:04o}, nlink={}, uid={}",
                    metadata.mode() & 0o7777,
                    metadata.nlink(),
                    metadata.uid(),
                ));
            }
            line
        }
        Err(error) => format!("metadata error: {error}"),
    }
}

/// The effective umask, measured by creating a probe file with an
/// unmasked request mode and reading back what the kernel kept.
fn umask_probe(dir: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let probe = dir.join(".umask-probe");
        let _ = fs::remove_file(&probe);
        match OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o666)
            .open(&probe)
        {
            Ok(file) => {
                let actual = file.metadata().map(|m| m.mode() & 0o777).ok();
                drop(file);
                let _ = fs::remove_file(&probe);
                match actual {
                    Some(actual) => format!("{:04o}", (!actual) & 0o666),
                    None => "unknown (metadata failed)".to_owned(),
                }
            }
            Err(error) => format!("probe failed: {error}"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        "n/a".to_owned()
    }
}

/// A full environment report for a failed (or retried) store open.
pub(crate) fn report(context: &str, workspace_root: &Path, error: StoreError) -> String {
    let drive = workspace_root.join(DRIVE_DIRECTORY);
    let database = drive.join(DATABASE_FILE);
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|e| format!("unknown ({e})"));
    format!(
        "[fluzo-diag] {context}\n\
         \x20 error: {error:?}\n\
         \x20 last driver connect error: {}\n\
         \x20 TMPDIR: {}\n\
         \x20 temp_dir: {}\n\
         \x20 cwd: {cwd}\n\
         \x20 umask: {}\n\
         \x20 root {}: {}\n\
         \x20 drive {}: {}\n\
         \x20 database {}: {}\n\
         \x20 wal exists: {}, shm exists: {}\n\
         \x20 pid: {}",
        crate::store::last_connect_error(),
        std::env::var("TMPDIR").unwrap_or_else(|_| "unset".to_owned()),
        std::env::temp_dir().display(),
        umask_probe(workspace_root),
        workspace_root.display(),
        describe(workspace_root),
        drive.display(),
        describe(&drive),
        database.display(),
        describe(&database),
        drive.join(format!("{DATABASE_FILE}-wal")).exists(),
        drive.join(format!("{DATABASE_FILE}-shm")).exists(),
        std::process::id(),
    )
}

fn append_to_log(root: &Path, text: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(FIXTURE_LOG))
    {
        let _ = writeln!(file, "{text}");
        let _ = file.sync_all();
    }
}

/// Print the report and persist it in the fixture's log file.
pub(crate) fn report_and_log(root: &Path, context: &str, error: StoreError) {
    let text = report(context, root, error);
    println!("{text}");
    append_to_log(root, &text);
}

/// Record that a retried open recovered after transient failures.
pub(crate) fn note_recovered(root: &Path, context: &str, attempt: u32) {
    let text =
        format!("[fluzo-diag] {context}: recovered on attempt {attempt} after transient failures");
    println!("{text}");
    append_to_log(root, &text);
}
