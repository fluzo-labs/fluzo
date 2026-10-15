//! DAT-02e: local telemetry archive purge with writer quiesce.
//!
//! Contract (PRD 22.2, Architecture A03, issue #76): this repository's
//! local diagnostic stores under `.fluzodrive/telemetry/` are purged as
//! two stores that are distinct from history (the DAT-01 journal):
//!
//! * the **diagnostic archive** — the bounded JSONL segments written by
//!   [`crate::telemetry_archive::JsonlArchive`] (`segment-*.jsonl`);
//! * the **pending export buffers** — the per-record diagnostic files
//!   captured through `Store::append_diagnostic`, which await a remote
//!   backend that OTLP export (#88) has not yet connected.
//!
//! The core safety property is quiescing before deletion: the purge stops
//! and drains the in-process diagnostic hub, or refuses with a typed
//! outcome naming the active component, and never deletes underneath a
//! live writer mid-append. A dry-run performs no deletion and no quiesce
//! and leaves the tree byte-for-byte unchanged.
//!
//! Records already exported to a remote backend stay under that backend's
//! ownership: the result states which local stores were cleared and which
//! data was not, and never claims to erase remote data. Traversal stays
//! within the managed telemetry root through the store's fd-anchored,
//! symlink-rejecting discipline; `.fluzo`, credentials and unrelated
//! volumes are never touched.

use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::quota::{
    DIAGNOSTICS_DIRECTORY, MountedFileSystem, QuotaClass, QuotaLedger, QuotaLimits,
};
use crate::store::{DRIVE_DIRECTORY, StoreError, anchor_class_in};
use crate::telemetry::DiagnosticHub;
use crate::telemetry_archive::{SEGMENT_PREFIX, SEGMENT_SUFFIX};

/// Whether the caller has confirmed a destructive purge. A dry-run never
/// deletes; `Yes` is required for deletion even in YOLO, and a
/// non-interactive caller that supplies neither deletes nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PurgeConfirmation {
    DryRun,
    Yes,
}

/// What a purge would remove, per store, with zero side effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgePreview {
    pub repository: String,
    pub telemetry_path: PathBuf,
    pub archive_segments: usize,
    pub archive_bytes: u64,
    pub pending_buffers: usize,
    pub pending_bytes: u64,
}

impl PurgePreview {
    pub fn total_bytes(&self) -> u64 {
        self.archive_bytes.saturating_add(self.pending_bytes)
    }
}

/// The outcome of a confirmed purge: which local stores were cleared, the
/// accounting released, and the data explicitly not cleared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgeResult {
    pub repository: String,
    pub telemetry_path: PathBuf,
    pub archive_segments_removed: usize,
    pub archive_bytes_removed: u64,
    pub pending_buffers_removed: usize,
    pub pending_bytes_removed: u64,
    /// DAT-02b diagnostics-class bytes measured before and after. The
    /// ledger recomputes from the filesystem, so the drop is structural.
    pub diagnostics_used_before: u64,
    pub diagnostics_used_after: u64,
    /// Data the purge does not and cannot clear, named so the result never
    /// overclaims. Remote backends and shared aggregates stay under their
    /// own ownership.
    pub not_cleared: Vec<String>,
}

impl fmt::Display for PurgeResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "Purged local telemetry for repository '{}' at {}",
            self.repository,
            self.telemetry_path.display()
        )?;
        writeln!(
            formatter,
            "  diagnostic archive: {} segment(s), {} byte(s) removed",
            self.archive_segments_removed, self.archive_bytes_removed
        )?;
        writeln!(
            formatter,
            "  pending export buffers: {} buffer(s), {} byte(s) removed",
            self.pending_buffers_removed, self.pending_bytes_removed
        )?;
        writeln!(
            formatter,
            "  DAT-02b diagnostics accounting: {} -> {} byte(s)",
            self.diagnostics_used_before, self.diagnostics_used_after
        )?;
        writeln!(formatter, "  not cleared (remote/shared ownership):")?;
        if self.not_cleared.is_empty() {
            writeln!(formatter, "    none")?;
        } else {
            for item in &self.not_cleared {
                writeln!(formatter, "    - {item}")?;
            }
        }
        Ok(())
    }
}

/// Typed purge failure. Every variant is machine-readable so a caller can
/// distinguish a missing confirmation from an active writer from an
/// unsafe root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PurgeError {
    /// A destructive purge was requested without `--yes`. Nothing was
    /// deleted. Confirmation is required even in YOLO.
    ConfirmationRequired,
    /// A diagnostic writer or exporter could not be quiesced within the
    /// deadline. Named so the operator knows what to stop. Nothing was
    /// deleted.
    ActiveWriter { component: String },
    /// The managed telemetry root is missing, symlinked or holds an entry
    /// outside the flat regular-file layout. Fails closed; nothing was
    /// deleted and nothing outside the root was touched.
    UnsafeRoot,
    /// A genuine I/O failure.
    Io(String),
}

impl fmt::Display for PurgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmationRequired => {
                formatter.write_str("purge requires explicit confirmation (--yes); nothing deleted")
            }
            Self::ActiveWriter { component } => {
                write!(
                    formatter,
                    "diagnostic component '{component}' is still active; refusing to delete under a live writer"
                )
            }
            Self::UnsafeRoot => formatter.write_str("managed telemetry root is unsafe"),
            Self::Io(message) => write!(formatter, "telemetry purge I/O failure: {message}"),
        }
    }
}

impl From<StoreError> for PurgeError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Inaccessible | StoreError::Conflict | StoreError::UnsupportedPath => {
                PurgeError::UnsafeRoot
            }
            other => PurgeError::Io(other.to_string()),
        }
    }
}

/// One classified on-disk entry under the managed telemetry root.
struct ClassifiedEntry {
    name: String,
    bytes: u64,
    archive: bool,
}

/// A scan of the managed telemetry root through an anchored handle.
struct TelemetryScan {
    entries: Vec<ClassifiedEntry>,
}

impl TelemetryScan {
    fn archive(&self) -> impl Iterator<Item = &ClassifiedEntry> {
        self.entries.iter().filter(|entry| entry.archive)
    }
    fn pending(&self) -> impl Iterator<Item = &ClassifiedEntry> {
        self.entries.iter().filter(|entry| !entry.archive)
    }
}

fn sum<'a>(entries: impl Iterator<Item = &'a ClassifiedEntry>) -> (usize, u64) {
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in entries {
        count += 1;
        bytes = bytes.saturating_add(entry.bytes);
    }
    (count, bytes)
}

/// True for the archive segment file names produced by `JsonlArchive`.
/// Anything else that is a managed regular file is a pending buffer.
fn is_archive_segment(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(SEGMENT_PREFIX) else {
        return false;
    };
    let Some(digits) = rest.strip_suffix(&format!(".{SEGMENT_SUFFIX}")) else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Anchors the managed telemetry directory read-only from the workspace
/// root. Returns `Ok(None)` when the drive or the telemetry directory has
/// not been created yet (nothing to purge), and `Err(UnsafeRoot)` when a
/// component is a symlink or not a directory. Never creates anything.
#[cfg(target_os = "linux")]
fn anchor_telemetry(workspace_root: &Path) -> Result<Option<(File, PathBuf)>, PurgeError> {
    if !workspace_root.is_absolute() {
        return Err(PurgeError::UnsafeRoot);
    }
    let drive_directory = workspace_root.join(DRIVE_DIRECTORY);
    // create = false: a missing drive is "nothing to purge", not an error,
    // and must not be created by a dry-run.
    let drive_handle = match crate::store::Store::anchor_drive_with(&drive_directory, false) {
        Ok(handle) => handle,
        Err(StoreError::Inaccessible) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let telemetry = match anchor_class_in(
        &drive_handle,
        &drive_directory,
        DIAGNOSTICS_DIRECTORY,
        false,
    ) {
        Ok(handle) => handle,
        Err(StoreError::Inaccessible) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some((telemetry, drive_directory)))
}

#[cfg(not(target_os = "linux"))]
fn anchor_telemetry(_workspace_root: &Path) -> Result<Option<(File, PathBuf)>, PurgeError> {
    Err(PurgeError::UnsafeRoot)
}

/// Reads the managed telemetry root through the anchored handle. Every
/// entry is inspected with `symlink_metadata` (never following a link);
/// a symlink, directory or any non-regular file fails the whole scan
/// closed so a tampered layout can never cause a delete outside the root.
fn scan_telemetry(telemetry: &File) -> Result<TelemetryScan, PurgeError> {
    use std::os::fd::AsRawFd;
    let dir_path = PathBuf::from(format!("/proc/self/fd/{}", telemetry.as_raw_fd()));
    let read = fs::read_dir(&dir_path).map_err(|error| PurgeError::Io(error.to_string()))?;
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(|error| PurgeError::Io(error.to_string()))?;
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|error| PurgeError::Io(error.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PurgeError::UnsafeRoot);
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let archive = is_archive_segment(&name);
        entries.push(ClassifiedEntry {
            name,
            bytes: metadata.len(),
            archive,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(TelemetryScan { entries })
}

fn repository_name(workspace_root: &Path) -> String {
    workspace_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Previews a local telemetry purge with zero side effects: no deletion,
/// no quiesce, no creation. Reports the diagnostic archive and the
/// pending export buffers that a confirmed purge would remove, naming the
/// repository and the exact `.fluzodrive/telemetry/` path with byte
/// counts.
pub fn preview(
    workspace_root: &Path,
    hub: Option<&DiagnosticHub>,
) -> Result<PurgePreview, PurgeError> {
    let telemetry_path = workspace_root
        .join(DRIVE_DIRECTORY)
        .join(DIAGNOSTICS_DIRECTORY);
    let (archive_segments, archive_bytes, pending_buffers, pending_bytes) =
        match anchor_telemetry(workspace_root)? {
            None => (0, 0, 0, 0),
            Some((telemetry, _drive)) => {
                let scan = scan_telemetry(&telemetry)?;
                let (segments, a_bytes) = sum(scan.archive());
                let (buffers, p_bytes) = sum(scan.pending());
                (segments, a_bytes, buffers, p_bytes)
            }
        };
    // The in-memory hub queues are the writer that a confirmed purge
    // quiesces; they are reported here as additional pending bytes so the
    // preview reflects everything a purge would settle, but reading them
    // is side-effect free.
    let mut pending_bytes = pending_bytes;
    let mut pending_buffers = pending_buffers;
    if let Some(hub) = hub {
        for health in hub.health() {
            pending_bytes = pending_bytes.saturating_add(health.queued_bytes as u64);
            pending_buffers = pending_buffers.saturating_add(health.queued_records);
        }
    }
    Ok(PurgePreview {
        repository: repository_name(workspace_root),
        telemetry_path,
        archive_segments,
        archive_bytes,
        pending_buffers,
        pending_bytes,
    })
}

/// Purges this repository's local diagnostic archive and pending export
/// buffers.
///
/// Order matters for the safety contract:
/// 1. A `DryRun` confirmation deletes nothing and returns
///    `ConfirmationRequired` rather than silently proceeding; the caller
///    must pass `Yes`.
/// 2. The managed root is scanned and validated *before* any quiesce, so
///    an unsafe root refuses without having stopped a writer.
/// 3. The in-process diagnostic hub is quiesced (admission stopped,
///    workers drained). A worker that cannot drain in time yields
///    `ActiveWriter` naming it, and nothing is deleted.
/// 4. Only then are the classified files removed through the anchored
///    handle.
///
/// `hub` is the writer to quiesce. A standalone CLI purge passes `None`
/// (no in-process writer); the quiesce-or-refuse path is exercised with
/// a live hub in the deterministic tests.
pub fn purge(
    workspace_root: &Path,
    hub: Option<&mut DiagnosticHub>,
    confirmation: PurgeConfirmation,
    deadline: Duration,
) -> Result<PurgeResult, PurgeError> {
    if confirmation != PurgeConfirmation::Yes {
        return Err(PurgeError::ConfirmationRequired);
    }
    let telemetry_path = workspace_root
        .join(DRIVE_DIRECTORY)
        .join(DIAGNOSTICS_DIRECTORY);
    let ledger = QuotaLedger::new(
        workspace_root.join(DRIVE_DIRECTORY),
        QuotaLimits::default(),
        MountedFileSystem,
    );
    let diagnostics_used_before = ledger.status(QuotaClass::Diagnostics)?.used_bytes;

    // Scan and validate the root before touching any writer. Keep the
    // anchored handle alive so removal stays inside it.
    let scan = match anchor_telemetry(workspace_root)? {
        None => TelemetryScan {
            entries: Vec::new(),
        },
        Some((telemetry, _drive)) => {
            let scan = scan_telemetry(&telemetry)?;
            // Quiesce the writer before deleting. A stuck sink refuses
            // the purge while nothing has been removed.
            if let Some(hub) = hub {
                let report = hub.shutdown(deadline);
                if let Some((component, _)) = report.incomplete_flushes.first() {
                    return Err(PurgeError::ActiveWriter {
                        component: component.clone(),
                    });
                }
            }
            remove_scanned(&telemetry, &scan)?;
            scan
        }
    };

    let (archive_segments_removed, archive_bytes_removed) = sum(scan.archive());
    let (pending_buffers_removed, pending_bytes_removed) = sum(scan.pending());

    let diagnostics_used_after = ledger.status(QuotaClass::Diagnostics)?.used_bytes;

    Ok(PurgeResult {
        repository: repository_name(workspace_root),
        telemetry_path,
        archive_segments_removed,
        archive_bytes_removed,
        pending_buffers_removed,
        pending_bytes_removed,
        diagnostics_used_before,
        diagnostics_used_after,
        not_cleared: vec![
            "remote backend copies already exported".to_owned(),
            "shared aggregates".to_owned(),
        ],
    })
}

/// Removes the scanned entries through the anchored telemetry handle.
/// Paths are built under `/proc/self/fd/{fd}` so the unlink happens
/// inside the verified directory, and each entry is re-checked as a
/// regular file immediately before unlink so a symlink introduced after
/// the scan is never followed.
#[cfg(target_os = "linux")]
fn remove_scanned(telemetry: &File, scan: &TelemetryScan) -> Result<(), PurgeError> {
    use std::os::fd::AsRawFd;
    let base = PathBuf::from(format!("/proc/self/fd/{}", telemetry.as_raw_fd()));
    for entry in &scan.entries {
        let path = base.join(&entry.name);
        let metadata = fs::symlink_metadata(&path).map_err(|e| PurgeError::Io(e.to_string()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(PurgeError::UnsafeRoot);
        }
        fs::remove_file(&path).map_err(|e| PurgeError::Io(e.to_string()))?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn remove_scanned(_telemetry: &File, _scan: &TelemetryScan) -> Result<(), PurgeError> {
    Err(PurgeError::UnsafeRoot)
}
