//! RUN-01a: workspace identity and cross-process writer ownership.
//!
//! PRD 21.2 requires at most one active writing task per workspace
//! across sessions and across Fluzo processes on the same host,
//! acquired before the execution baseline and retained across dependent
//! edits, verification commands and approval waits.
//!
//! The SQLite write lock cannot provide that. `commit_unit` takes
//! `BEGIN IMMEDIATE` and releases it at commit, so between two commits
//! nothing is held -- and an approval wait is exactly the interval
//! during which nothing must be releasable. Ownership here is therefore
//! a separate, deliberately held resource.
//!
//! The primitive is an exclusive advisory lock on a descriptor held for
//! the whole ownership period. It is the only thing in this design that
//! actually excludes another process: a durable row alone is useless,
//! because reading it and then writing it is itself a race. The kernel
//! releases the lock when the owning process dies, which is what makes a
//! crashed owner recoverable at all. The record written inside the file
//! is reconciliation input and a diagnostic, not the lock.
//!
//! Advisory means advisory: this excludes cooperating Fluzo processes.
//! It does not stop a program that does not take the lock, and nothing
//! here is an OS sandbox.

use crate::store::{ANCHOR_FLAGS, DRIVE_DIRECTORY, NOFOLLOW_CLOEXEC, StoreError};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Owned by the writer-ownership contract. Lives directly under
/// `.fluzodrive` at a fixed name so content scans never mistake it for
/// purgeable data.
pub const WRITER_LOCK_FILE: &str = "writer.lock";

/// Record format marker. A reader that does not recognise it refuses
/// rather than guessing at the layout.
const RECORD_HEADER: &str = "fluzo-writer-lock v1";

/// Hex characters kept from the identity hash. Long enough that two
/// workspaces do not collide in practice, short enough to read.
const FINGERPRINT_CHARS: usize = 16;

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn map_store_error(error: StoreError) -> OwnershipError {
    match error {
        StoreError::UnsupportedPath => OwnershipError::UnsupportedPath,
        StoreError::Inaccessible => OwnershipError::Inaccessible,
        StoreError::Conflict => OwnershipError::Conflict,
        _ => OwnershipError::Io,
    }
}

/// Errors of the identity and ownership contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnershipError {
    /// The root is not an absolute, normalized path this contract will
    /// operate on.
    UnsupportedPath,
    /// The root or the drive could not be reached.
    Inaccessible,
    /// A component changed between resolution and the anchored open.
    /// Fails closed rather than opening something else.
    AnchorLost,
    /// The lock path exists but is not a regular file owned by this
    /// contract, for example a symlink.
    Conflict,
    /// Another process holds the workspace. The holder record is the
    /// diagnostic the operator needs; the acquire dispatched nothing.
    HeldBy { holder: Box<OwnerRecord> },
    /// The lock is held but the record inside it could not be read. The
    /// workspace is still refused: an unreadable holder is not an
    /// absent one.
    HeldUnknown,
    /// The lock file exists and is free, but its record is malformed.
    /// Reported distinctly so a corrupt record is not confused with a
    /// live holder or with an empty drive.
    CorruptRecord { reason: String },
    /// A required durability step on the record failed.
    Io,
}

impl std::fmt::Display for OwnershipError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPath => formatter.write_str("unsupported workspace path"),
            Self::Inaccessible => formatter.write_str("workspace is inaccessible"),
            Self::AnchorLost => {
                formatter.write_str("workspace root changed while it was being anchored")
            }
            Self::Conflict => formatter.write_str("writer lock path conflicts with a safe owner"),
            Self::HeldBy { holder } => write!(
                formatter,
                "workspace is already owned by process {} (owner {})",
                holder.pid, holder.owner
            ),
            Self::HeldUnknown => {
                formatter.write_str("workspace is owned by an unreadable holder record")
            }
            Self::CorruptRecord { reason } => {
                write!(formatter, "writer lock record is malformed: {reason}")
            }
            Self::Io => formatter.write_str("writer lock I/O failure"),
        }
    }
}

/// The resolved root plus the filesystem identity of the directory it
/// resolves to.
///
/// Identity is read from an anchored descriptor, never from the path
/// string: two different strings that reach the same directory agree,
/// and a path that was rewritten between calls does not silently pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    root: PathBuf,
    dev: u64,
    ino: u64,
}

impl WorkspaceIdentity {
    /// Resolves the workspace root and reads its filesystem identity.
    ///
    /// The root is resolved first because PRD 21.2 asks for the
    /// resolved working-tree root. The anchored walk that follows still
    /// refuses to follow a symlink that appeared in the meantime: it
    /// fails closed rather than opening whatever the name now points at.
    pub fn resolve(root: &Path) -> Result<Self, OwnershipError> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err(OwnershipError::UnsupportedPath);
        }
        let resolved = root
            .canonicalize()
            .map_err(|_| OwnershipError::Inaccessible)?;
        if !resolved.is_absolute()
            || resolved
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(OwnershipError::UnsupportedPath);
        }
        let handle = anchor_from_root(&resolved)?;
        let metadata = handle
            .metadata()
            .map_err(|_| OwnershipError::Inaccessible)?;
        Ok(Self {
            root: resolved,
            dev: metadata.dev(),
            ino: metadata.ino(),
        })
    }

    /// The resolved root. Kept for the runtime's own use; it must not go
    /// into a diagnostic trace by default (PRD 21.1).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A stable, path-free identifier for traces and records.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.dev.to_le_bytes());
        hasher.update(self.ino.to_le_bytes());
        hex_lower(&hasher.finalize())[..FINGERPRINT_CHARS].to_owned()
    }

    /// Filesystem device of the resolved root.
    pub fn dev(&self) -> u64 {
        self.dev
    }

    /// Filesystem inode of the resolved root.
    pub fn ino(&self) -> u64 {
        self.ino
    }
}

/// Walks from `/` to `resolved` opening each component with
/// `O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC`, so the returned descriptor
/// cannot have been led through a symlink by another process mid-walk.
fn anchor_from_root(resolved: &Path) -> Result<File, OwnershipError> {
    let mut directory = File::open("/").map_err(|_| OwnershipError::Inaccessible)?;
    for component in resolved.components() {
        if let Component::Normal(name) = component {
            let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
            directory = OpenOptions::new()
                .read(true)
                .custom_flags(ANCHOR_FLAGS)
                .open(path)
                .map_err(|_| OwnershipError::AnchorLost)?;
        }
    }
    Ok(directory)
}

/// What the current holder recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerRecord {
    /// Process that acquired the workspace.
    pub pid: u32,
    /// Wall-clock milliseconds of the acquisition.
    pub acquired_ms: u64,
    /// Fingerprint of the workspace this record belongs to, so a copied
    /// lock file cannot impersonate another workspace.
    pub workspace: String,
    /// Identifies this particular acquisition, distinct from the pid
    /// because pids are reused.
    pub owner: String,
    /// Set when this acquisition found a record left by a holder whose
    /// lock was no longer held, i.e. a crashed owner that was
    /// reconciled rather than replaced.
    pub reconciled_from: Option<String>,
    /// Set by a clean release. A record without it whose lock is free
    /// means the holder died.
    pub released_ms: Option<u64>,
}

impl OwnerRecord {
    fn render(&self) -> String {
        format!(
            "{RECORD_HEADER}\npid={}\nacquired_ms={}\nworkspace={}\nowner={}\nreconciled_from={}\nreleased_ms={}\n",
            self.pid,
            self.acquired_ms,
            self.workspace,
            self.owner,
            self.reconciled_from
                .clone()
                .unwrap_or_else(|| "none".to_owned()),
            self.released_ms
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_owned()),
        )
    }

    fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default();
        if header != RECORD_HEADER {
            return Err(format!("unexpected header {header:?}"));
        }
        let mut fields: Vec<(String, String)> = Vec::new();
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("malformed line {line:?}"));
            };
            fields.push((key.to_owned(), value.to_owned()));
        }
        let take = |name: &str| -> Result<String, String> {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| format!("missing field {name:?}"))
        };
        let optional = |name: &str| -> Result<Option<String>, String> {
            let value = take(name)?;
            Ok(if value == "none" { None } else { Some(value) })
        };
        let pid = take("pid")?
            .parse::<u32>()
            .map_err(|_| "pid is not a number".to_owned())?;
        let acquired_ms = take("acquired_ms")?
            .parse::<u64>()
            .map_err(|_| "acquired_ms is not a number".to_owned())?;
        let released_ms = optional("released_ms")?
            .map(|value| {
                value
                    .parse::<u64>()
                    .map_err(|_| "released_ms is not a number".to_owned())
            })
            .transpose()?;
        Ok(Self {
            pid,
            acquired_ms,
            workspace: take("workspace")?,
            owner: take("owner")?,
            reconciled_from: optional("reconciled_from")?,
            released_ms,
        })
    }
}

/// Identifies one acquisition. Not a security token: pids are reused,
/// so the record needs something that distinguishes this acquisition
/// from a later one by a recycled pid.
fn acquisition_nonce(identity: &WorkspaceIdentity) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.dev().to_le_bytes());
    hasher.update(identity.ino().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(now_ms().to_le_bytes());
    hex_lower(&hasher.finalize())[..FINGERPRINT_CHARS].to_owned()
}

/// Refusal returned together with the ownership so the caller cannot
/// lose the handle and leave the workspace neither owned nor released.
#[derive(Debug)]
pub struct ReleaseRefusal {
    pub outstanding: u32,
    pub uncertain: u32,
    pub ownership: WriterOwnership,
}

impl std::fmt::Display for ReleaseRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "cannot release: {} mutating operation(s) outstanding and {} uncertain writer(s); the workspace stays owned",
            self.outstanding, self.uncertain
        )
    }
}

/// Proof of a clean release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseReceipt {
    pub owner: String,
    pub released_ms: u64,
}

/// A counted mutating operation. Held by the code doing the write.
pub struct WriterTicket {
    outstanding: Arc<AtomicU32>,
    uncertain: Arc<AtomicU32>,
}

impl WriterTicket {
    /// Reports the operation finished. The count drops when the ticket
    /// is dropped.
    pub fn completed(self) {
        drop(self);
    }

    /// Reports the operation's outcome could not be confirmed. The
    /// uncertain count survives the ticket: release stays refused until
    /// something reconciles the uncertainty, because "we do not know
    /// whether a write is in flight" is not "nothing is in flight".
    pub fn uncertain(self) {
        self.uncertain.fetch_add(1, Ordering::SeqCst);
        // This ticket's outstanding slot is released, but the refusal
        // is kept by the uncertain count.
        self.outstanding.fetch_sub(1, Ordering::SeqCst);
        std::mem::forget(self);
    }
}

impl Drop for WriterTicket {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Result of asking, without acquiring, who owns the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OwnerProbe {
    /// No lock file, or a lock file with no record in it.
    Free,
    /// A live holder has the exclusive lock.
    Held(Box<OwnerRecord>),
    /// The lock is held but the record could not be read.
    HeldUnknown,
    /// The last recorded holder released cleanly.
    Released(Box<OwnerRecord>),
    /// A record with no release marker sits on a lock nobody holds.
    /// Reported rather than treated as free, because it means the
    /// previous holder died and an operator should see that.
    Abandoned(Box<OwnerRecord>),
}

/// A workspace this process holds writer ownership of.
///
/// The lock is held by the descriptor, not by a flag: it is released
/// when [`WriterOwnership::release`] completes or when the process
/// exits. Every field is owned by this value; there is no shared global
/// that another component can clear.
#[derive(Debug)]
pub struct WriterOwnership {
    lock_file: File,
    drive_handle: File,
    drive_directory: PathBuf,
    identity: WorkspaceIdentity,
    record: OwnerRecord,
    /// Live mutating operations. A ticket decrements on drop, so a
    /// forgotten ticket keeps blocking release rather than silently
    /// permitting it.
    outstanding: Arc<AtomicU32>,
    /// Operations whose outcome could not be confirmed. Never
    /// decremented: an uncertain writer is not a quiescent one.
    uncertain: Arc<AtomicU32>,
}

impl WriterOwnership {
    /// Acquires exclusive writer ownership of `root`.
    ///
    /// On success the exclusive lock is held and the record is durable.
    /// On refusal nothing was written and nothing was dispatched.
    pub fn acquire(root: &Path) -> Result<Self, OwnershipError> {
        let identity = WorkspaceIdentity::resolve(root)?;
        let drive_directory = identity.root().join(DRIVE_DIRECTORY);
        let drive_handle = crate::store::Store::anchor_drive_with(&drive_directory, true)
            .map_err(map_store_error)?;
        let lock_file = Self::open_lock_file(&drive_handle)?;

        match rustix::fs::flock(
            &lock_file,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return match Self::read_record(&lock_file) {
                    Ok(Some(record)) => Err(OwnershipError::HeldBy {
                        holder: Box::new(record),
                    }),
                    Ok(None) | Err(_) => Err(OwnershipError::HeldUnknown),
                };
            }
            Err(_) => return Err(OwnershipError::Io),
        }

        // We hold the exclusive lock, so any record we now read belongs
        // to a holder that no longer holds it. That is a reconciliation,
        // not a replacement, and it is recorded as one. A record that
        // was cleanly released is a normal handoff, not a reconciliation.
        let previous = Self::read_record(&lock_file)?;
        if let Some(record) = &previous {
            // A record naming a different workspace means the lock file
            // was copied or moved here. Refuse rather than reconcile
            // against a record that describes some other directory.
            if record.workspace != identity.fingerprint() {
                return Err(OwnershipError::CorruptRecord {
                    reason: format!(
                        "record names workspace {}, not {}",
                        record.workspace,
                        identity.fingerprint()
                    ),
                });
            }
        }
        let reconciled_from = match &previous {
            Some(record) if record.released_ms.is_none() => Some(record.owner.clone()),
            _ => None,
        };
        let record = OwnerRecord {
            pid: std::process::id(),
            acquired_ms: now_ms(),
            workspace: identity.fingerprint(),
            owner: acquisition_nonce(&identity),
            reconciled_from,
            released_ms: None,
        };
        Self::write_record(&lock_file, &record)?;
        Ok(Self {
            lock_file,
            drive_handle,
            drive_directory,
            identity,
            record,
            outstanding: Arc::new(AtomicU32::new(0)),
            uncertain: Arc::new(AtomicU32::new(0)),
        })
    }

    /// Opens the lock file through the anchored drive handle. A symlink
    /// at the lock path is refused rather than followed, so another
    /// actor in the directory cannot point the lock somewhere else.
    fn open_lock_file(drive_handle: &File) -> Result<File, OwnershipError> {
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", drive_handle.as_raw_fd()))
            .join(WRITER_LOCK_FILE);
        match fs::symlink_metadata(&anchored) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(OwnershipError::Conflict);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(OwnershipError::Inaccessible),
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .custom_flags(NOFOLLOW_CLOEXEC)
            .mode(0o600)
            .open(anchored)
            .map_err(|_| OwnershipError::Conflict)
    }

    fn read_record(lock_file: &File) -> Result<Option<OwnerRecord>, OwnershipError> {
        let mut file = lock_file.try_clone().map_err(|_| OwnershipError::Io)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| OwnershipError::Io)?;
        let mut text = String::new();
        match file.read_to_string(&mut text) {
            Ok(0) => return Ok(None),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                return Err(OwnershipError::CorruptRecord {
                    reason: "not valid UTF-8".to_owned(),
                });
            }
            Err(_) => return Err(OwnershipError::Io),
        }
        if text.trim().is_empty() {
            return Ok(None);
        }
        OwnerRecord::parse(&text)
            .map(Some)
            .map_err(|reason| OwnershipError::CorruptRecord { reason })
    }

    fn write_record(lock_file: &File, record: &OwnerRecord) -> Result<(), OwnershipError> {
        let mut file = lock_file.try_clone().map_err(|_| OwnershipError::Io)?;
        // Truncate before writing so a longer previous record cannot
        // leave trailing bytes that a later parse would reject.
        file.set_len(0).map_err(|_| OwnershipError::Io)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| OwnershipError::Io)?;
        file.write_all(record.render().as_bytes())
            .map_err(|_| OwnershipError::Io)?;
        // The record must be durable before the acquisition is reported:
        // a reconciliation that is lost is indistinguishable from one
        // that never happened.
        file.sync_all().map_err(|_| OwnershipError::Io)
    }

    /// The identity of the workspace held.
    pub fn identity(&self) -> &WorkspaceIdentity {
        &self.identity
    }

    /// This acquisition's record, as written.
    pub fn record(&self) -> &OwnerRecord {
        &self.record
    }

    /// Counts one mutating operation. The returned ticket must be held
    /// for the duration of the operation.
    pub fn begin_mutation(&self) -> WriterTicket {
        self.outstanding.fetch_add(1, Ordering::SeqCst);
        WriterTicket {
            outstanding: Arc::clone(&self.outstanding),
            uncertain: Arc::clone(&self.uncertain),
        }
    }

    /// Live mutating operations.
    pub fn outstanding(&self) -> u32 {
        self.outstanding.load(Ordering::SeqCst)
    }

    /// Operations whose outcome was reported unconfirmable.
    pub fn uncertain(&self) -> u32 {
        self.uncertain.load(Ordering::SeqCst)
    }

    /// Re-checks that the drive this ownership was built on is still the
    /// directory at that path. A drive replaced underneath the owner is
    /// a `Conflict`: the lock we hold is on a file that is no longer
    /// where the workspace is.
    pub fn verify_still_owned(&self) -> Result<(), OwnershipError> {
        let current = fs::symlink_metadata(&self.drive_directory)
            .map_err(|_| OwnershipError::Inaccessible)?;
        let held = self
            .drive_handle
            .metadata()
            .map_err(|_| OwnershipError::Inaccessible)?;
        if !current.is_dir() || current.dev() != held.dev() || current.ino() != held.ino() {
            return Err(OwnershipError::Conflict);
        }
        Ok(())
    }

    /// Releases ownership at a quiescent checkpoint.
    ///
    /// Refuses while any mutating operation is outstanding or any
    /// uncertain writer remains, and returns the ownership unchanged so
    /// the caller cannot end up holding neither a lock nor a receipt.
    /// On success the release marker is durable before the lock is
    /// dropped, and the descriptor is closed here rather than left to a
    /// deferred drop.
    pub fn release(self) -> Result<ReleaseReceipt, Box<ReleaseRefusal>> {
        let outstanding = self.outstanding.load(Ordering::SeqCst);
        let uncertain = self.uncertain.load(Ordering::SeqCst);
        if outstanding > 0 || uncertain > 0 {
            return Err(Box::new(ReleaseRefusal {
                outstanding,
                uncertain,
                ownership: self,
            }));
        }
        let released_ms = now_ms();
        let mut marker = self.record.clone();
        marker.released_ms = Some(released_ms);
        // A failed durable marker must not be reported as a clean
        // release: the next acquirer would see the un-released record
        // and call this a crash reconciliation. The ownership is handed
        // back so the lock is never lost on the error path.
        if Self::write_record(&self.lock_file, &marker).is_err() {
            return Err(Box::new(ReleaseRefusal {
                outstanding: 0,
                uncertain: 0,
                ownership: self,
            }));
        }
        let receipt = ReleaseReceipt {
            owner: self.record.owner.clone(),
            released_ms,
        };
        // Explicit unlock and explicit close. Closing the descriptor is
        // what releases the lock; doing it here rather than in Drop
        // keeps the release point visible and ordered after the durable
        // marker.
        let _ = rustix::fs::flock(&self.lock_file, rustix::fs::FlockOperation::Unlock);
        drop(self.lock_file);
        Ok(receipt)
    }

    /// Reports who owns the workspace without acquiring it.
    ///
    /// A shared lock is attempted so the record read is not torn. If the
    /// shared lock cannot be taken, an exclusive holder is present and the
    /// record is read best-effort for the diagnostic.
    pub fn probe(root: &Path) -> Result<OwnerProbe, OwnershipError> {
        let identity = WorkspaceIdentity::resolve(root)?;
        let drive_directory = identity.root().join(DRIVE_DIRECTORY);
        let drive_handle = crate::store::Store::anchor_drive_with(&drive_directory, false)
            .map_err(map_store_error)?;
        let anchored = PathBuf::from(format!("/proc/self/fd/{}", drive_handle.as_raw_fd()))
            .join(WRITER_LOCK_FILE);
        match fs::symlink_metadata(&anchored) {
            // No lock file at all: nobody has ever claimed this drive.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(OwnerProbe::Free);
            }
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(OwnershipError::Conflict);
                }
            }
            Err(_) => return Err(OwnershipError::Inaccessible),
        }
        let lock_file = OpenOptions::new()
            .read(true)
            .custom_flags(NOFOLLOW_CLOEXEC)
            .open(anchored)
            .map_err(|_| OwnershipError::Conflict)?;
        let shared = rustix::fs::flock(
            &lock_file,
            rustix::fs::FlockOperation::NonBlockingLockShared,
        );
        let record = Self::read_record(&lock_file)?;
        match shared {
            Ok(()) => {
                let _ = rustix::fs::flock(&lock_file, rustix::fs::FlockOperation::Unlock);
                match record {
                    None => Ok(OwnerProbe::Free),
                    Some(record) if record.released_ms.is_some() => {
                        Ok(OwnerProbe::Released(Box::new(record)))
                    }
                    Some(record) => Ok(OwnerProbe::Abandoned(Box::new(record))),
                }
            }
            Err(_) => match record {
                Some(record) => Ok(OwnerProbe::Held(Box::new(record))),
                None => Ok(OwnerProbe::HeldUnknown),
            },
        }
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
