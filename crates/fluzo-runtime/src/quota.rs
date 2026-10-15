//! DAT-02b storage quotas and admission guardrails.
//!
//! Contract (PRD 22.2, Architecture A03, issue #73): this repository's
//! `.fluzodrive/` contents are accounted into two independent classes,
//! session data (the DAT-01 journal with its WAL side files plus the
//! DAT-02a artifact tree) and local diagnostics. Limits come only from
//! this repository's `.fluzo` typed settings, so changing one project's
//! quota reconfigures and purges nothing elsewhere.
//!
//! Enforcement is admission-time only, never a hard filesystem quota:
//! in-flight writes, SQLite journals and filesystem overhead need
//! headroom that this module cannot reserve. At the session threshold the
//! ledger refuses new state-mutating work before the audit state that
//! work requires could become unrecordable. At the diagnostic threshold
//! existing capture is retained and only new writes are dropped, with
//! counters that make the degradation visible instead of silent.
//!
//! Nothing here deletes, evicts or expires retained data. There is no
//! clock dependency for exactly that reason: elapsed time is not an
//! input to any decision, so time passing cannot remove a record.
//! Cleanup is an explicit operation owned by DAT-02d.

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use fluzo_core::settings::StorageSettings;

use crate::artifact::ARTIFACTS_DIRECTORY;
use crate::store::{DATABASE_FILE, DRIVE_DIRECTORY, StoreError};

/// Managed local diagnostic archive inside `.fluzodrive/`.
pub const DIAGNOSTICS_DIRECTORY: &str = "telemetry";
/// Utilization at which a typed warning is surfaced, before refusal.
pub const WARNING_THRESHOLD_PERCENT: u8 = 80;
/// Bytes kept free inside the session limit for the audit state of the
/// unit being admitted. A unit is refused while `used + incoming + this`
/// would reach the limit, so the refusal happens before the required
/// audit rows are at risk rather than during them.
pub const DEFAULT_AUDIT_HEADROOM_BYTES: u64 = 64 * 1024;
/// Safety bound on how many entries one accounted class may hold. Far
/// above any real store; exceeding it is an anomaly, not a slow path.
const MAX_ACCOUNTED_ENTRIES: usize = 1_000_000;

/// Which accounted class a measurement, limit or decision concerns.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum QuotaClass {
    /// Journal, WAL side files and finalized artifacts.
    SessionData,
    /// Local diagnostic archive.
    Diagnostics,
}

/// Effective per-repository limits. Constructed from the typed settings
/// so the PRD defaults live in one place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaLimits {
    pub session_max_bytes: u64,
    pub diagnostic_max_bytes: u64,
    pub min_free_bytes: u64,
}

impl QuotaLimits {
    /// Maps this repository's `.fluzo` storage settings onto the ledger.
    pub const fn from_storage(settings: &StorageSettings) -> Self {
        Self {
            session_max_bytes: settings.session_max_bytes,
            diagnostic_max_bytes: settings.diagnostic_max_bytes,
            min_free_bytes: settings.min_free_bytes,
        }
    }

    pub const fn limit_for(&self, class: QuotaClass) -> u64 {
        match class {
            QuotaClass::SessionData => self.session_max_bytes,
            QuotaClass::Diagnostics => self.diagnostic_max_bytes,
        }
    }
}

impl Default for QuotaLimits {
    fn default() -> Self {
        Self::from_storage(&StorageSettings::default())
    }
}

/// Measured bytes per class, recomputed from the managed root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QuotaUsage {
    pub session_bytes: u64,
    pub diagnostics_bytes: u64,
}

impl QuotaUsage {
    pub const fn of(&self, class: QuotaClass) -> u64 {
        match class {
            QuotaClass::SessionData => self.session_bytes,
            QuotaClass::Diagnostics => self.diagnostics_bytes,
        }
    }
}

/// One class's position against its limit at a point in time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaStatus {
    pub class: QuotaClass,
    pub used_bytes: u64,
    pub limit_bytes: u64,
    /// Set at or above `WARNING_THRESHOLD_PERCENT` utilization. A warning
    /// is an observable outcome, not yet a refusal.
    pub warning: bool,
}

/// Machine-readable reason an admission did not succeed. Kept distinct
/// from every other failure category so a caller can tell "quota reached"
/// apart from an I/O or protocol problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaViolation {
    /// This repository's session-data limit is reached.
    SessionLimit,
    /// This repository's diagnostics limit is reached.
    DiagnosticLimit,
    /// The hosting filesystem is below the admission reserve. This is a
    /// safety check on a potentially shared disk, not an exclusive
    /// reservation.
    FreeSpaceReserve,
}

/// A refusal with the numbers behind it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaRefusal {
    pub violation: QuotaViolation,
    pub status: QuotaStatus,
    /// Bytes the refused operation would have added.
    pub projected_bytes: u64,
}

/// Typed outcome of a session-data admission request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    /// Below the warning threshold.
    Admitted(QuotaStatus),
    /// At or above the warning threshold but still admitted. Surfaced
    /// before any refusal happens.
    Warning(QuotaStatus),
    /// Not admitted. The caller must not start the state-mutating work.
    Refused(QuotaRefusal),
}

impl Admission {
    pub const fn admitted(&self) -> bool {
        matches!(self, Self::Admitted(_) | Self::Warning(_))
    }

    pub const fn status(&self) -> QuotaStatus {
        match self {
            Self::Admitted(status) | Self::Warning(status) => *status,
            Self::Refused(refusal) => refusal.status,
        }
    }
}

/// Typed outcome of a diagnostic capture request. Unlike session
/// admission this never blocks ordinary execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticDisposition {
    /// The record may be written.
    Accepted(QuotaStatus),
    /// The record must not be written. Existing capture is retained.
    Dropped {
        reason: QuotaViolation,
        status: QuotaStatus,
    },
}

impl DiagnosticDisposition {
    pub const fn accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }
}

/// Available bytes on the filesystem hosting a path. Injectable so the
/// reserve refusal path is deterministic without filling a real disk.
pub trait FileSystemProbe: std::fmt::Debug {
    fn available_bytes(&self, path: &Path) -> Result<u64, StoreError>;
}

/// Real probe over the mounted filesystem, using `statvfs`.
#[derive(Clone, Copy, Debug)]
pub struct MountedFileSystem;

impl FileSystemProbe for MountedFileSystem {
    fn available_bytes(&self, path: &Path) -> Result<u64, StoreError> {
        #[cfg(unix)]
        {
            let stat = rustix::fs::statvfs(path).map_err(|_| StoreError::Inaccessible)?;
            let fragment = match stat.f_frsize {
                0 => stat.f_bsize,
                other => other,
            };
            // `f_bavail` is the block count usable by an unprivileged
            // process, which is the correct basis for a reserve check.
            Ok((stat.f_bavail as u64).saturating_mul(fragment as u64))
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(StoreError::UnsupportedPath)
        }
    }
}

/// Deterministic probe for tests and fixtures.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct FixedFreeSpace(pub u64);

#[cfg(test)]
impl FileSystemProbe for FixedFreeSpace {
    fn available_bytes(&self, _path: &Path) -> Result<u64, StoreError> {
        Ok(self.0)
    }
}

/// Per-repository quota ledger bound to one managed drive root.
#[derive(Debug)]
pub struct QuotaLedger {
    drive_directory: PathBuf,
    limits: QuotaLimits,
    probe: Box<dyn FileSystemProbe + Send + Sync>,
}

impl QuotaLedger {
    pub fn new(
        drive_directory: PathBuf,
        limits: QuotaLimits,
        probe: impl FileSystemProbe + Send + Sync + 'static,
    ) -> Self {
        Self {
            drive_directory,
            limits,
            probe: Box::new(probe),
        }
    }

    pub fn limits(&self) -> QuotaLimits {
        self.limits
    }

    /// Replaces the effective limits on an existing ledger. Used by the
    /// store's test hook so a test can derive its thresholds from the
    /// real size of a freshly created journal instead of hardcoding a
    /// figure that changes with SQLite internals.
    #[cfg(test)]
    pub(crate) fn set_limits(&mut self, limits: QuotaLimits) {
        self.limits = limits;
    }

    pub fn status(&self, class: QuotaClass) -> Result<QuotaStatus, StoreError> {
        let used = self.measure()?.of(class);
        let limit = self.limits.limit_for(class);
        Ok(QuotaStatus {
            class,
            used_bytes: used,
            limit_bytes: limit,
            warning: reached_fraction(used, limit, WARNING_THRESHOLD_PERCENT),
        })
    }

    /// Free bytes on the hosting filesystem. Returns the measured value
    /// even when it is short; callers decide what a shortfall means for
    /// the class they are admitting.
    pub fn reserve_available(&self) -> Result<u64, StoreError> {
        self.probe.available_bytes(&self.drive_directory)
    }

    /// Admission for state-mutating session work.
    ///
    /// The reserve is checked first because a short filesystem endangers
    /// the audit record itself. The class limit is then checked against
    /// the projected total including the audit headroom, so refusal
    /// happens before the unit's own audit state is at risk.
    pub fn admit_session(&self, incoming_bytes: u64) -> Result<Admission, StoreError> {
        self.admit_session_with_headroom(incoming_bytes, DEFAULT_AUDIT_HEADROOM_BYTES)
    }

    pub fn admit_session_with_headroom(
        &self,
        incoming_bytes: u64,
        audit_headroom_bytes: u64,
    ) -> Result<Admission, StoreError> {
        let status = self.status(QuotaClass::SessionData)?;
        if self.reserve_shortfall()?.is_some() {
            return Ok(Admission::Refused(QuotaRefusal {
                violation: QuotaViolation::FreeSpaceReserve,
                status,
                projected_bytes: incoming_bytes,
            }));
        }
        let projected = incoming_bytes
            .saturating_add(audit_headroom_bytes)
            .saturating_add(status.used_bytes);
        if projected >= status.limit_bytes {
            return Ok(Admission::Refused(QuotaRefusal {
                violation: QuotaViolation::SessionLimit,
                status,
                projected_bytes: incoming_bytes,
            }));
        }
        if status.warning {
            return Ok(Admission::Warning(status));
        }
        Ok(Admission::Admitted(status))
    }

    /// Admission for local diagnostic capture.
    ///
    /// A shortfall here drops the new record instead of refusing work:
    /// diagnostics must not block ordinary execution. Existing capture
    /// is never touched.
    pub fn admit_diagnostic(
        &self,
        incoming_bytes: u64,
    ) -> Result<DiagnosticDisposition, StoreError> {
        let status = self.status(QuotaClass::Diagnostics)?;
        if self.reserve_shortfall()?.is_some() {
            return Ok(DiagnosticDisposition::Dropped {
                reason: QuotaViolation::FreeSpaceReserve,
                status,
            });
        }
        let projected = incoming_bytes.saturating_add(status.used_bytes);
        if projected >= status.limit_bytes {
            return Ok(DiagnosticDisposition::Dropped {
                reason: QuotaViolation::DiagnosticLimit,
                status,
            });
        }
        Ok(DiagnosticDisposition::Accepted(status))
    }

    fn reserve_shortfall(&self) -> Result<Option<u64>, StoreError> {
        let available = self.reserve_available()?;
        if available < self.limits.min_free_bytes {
            Ok(Some(available))
        } else {
            Ok(None)
        }
    }

    /// Recomputes both classes from the managed root.
    ///
    /// Only the managed entries are read, each with `symlink_metadata`, so
    /// a symlinked class directory or file is reported as a conflict
    /// rather than counted through into whatever it points at.
    pub fn measure(&self) -> Result<QuotaUsage, StoreError> {
        let session_bytes = self.database_bytes()? + self.class_bytes(ARTIFACTS_DIRECTORY)?.0;
        let diagnostics_bytes = self.class_bytes(DIAGNOSTICS_DIRECTORY)?.0;
        Ok(QuotaUsage {
            session_bytes,
            diagnostics_bytes,
        })
    }

    /// Bytes of the journal plus its WAL side files. A missing database
    /// measures zero: an unopened store holds nothing.
    fn database_bytes(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for name in [
            DATABASE_FILE,
            &format!("{DATABASE_FILE}-wal"),
            &format!("{DATABASE_FILE}-shm"),
        ] {
            let path = self.drive_directory.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !is_regular(&metadata) {
                        return Err(StoreError::Conflict);
                    }
                    total = total.saturating_add(metadata.len());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(StoreError::Inaccessible),
            }
        }
        Ok(total)
    }

    /// Flat bytes directly under one managed class directory. Returns the
    /// total and the entry count. An absent directory measures zero.
    fn class_bytes(&self, class_dir: &str) -> Result<(u64, usize), StoreError> {
        let directory = self.drive_directory.join(class_dir);
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((0, 0)),
            Err(_) => Err(StoreError::Inaccessible),
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(StoreError::Conflict);
                }
                let mut total = 0u64;
                let mut entries = 0usize;
                let read = fs::read_dir(&directory).map_err(|_| StoreError::Inaccessible)?;
                for entry in read {
                    let entry = entry.map_err(|_| StoreError::Inaccessible)?;
                    let metadata = entry
                        .path()
                        .symlink_metadata()
                        .map_err(|_| StoreError::Inaccessible)?;
                    if !is_regular(&metadata) {
                        // Nested trees, sockets and symlinks are outside
                        // the managed flat layout.
                        return Err(StoreError::Conflict);
                    }
                    total = total.saturating_add(metadata.len());
                    entries += 1;
                    if entries > MAX_ACCOUNTED_ENTRIES {
                        return Err(StoreError::Conflict);
                    }
                }
                Ok((total, entries))
            }
        }
    }
}

fn is_regular(metadata: &Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink()
}

/// Exact fraction test without floating point or overflow.
fn reached_fraction(used: u64, limit: u64, percent: u8) -> bool {
    if limit == 0 {
        return true;
    }
    u128::from(used) * 100 >= u128::from(limit) * u128::from(percent)
}

/// Path of the managed drive root for a workspace, shared with the store.
pub fn drive_directory_of(workspace_root: &Path) -> PathBuf {
    workspace_root.join(DRIVE_DIRECTORY)
}

#[cfg(test)]
#[path = "quota_tests.rs"]
mod quota_tests;
