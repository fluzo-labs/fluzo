//! RUN-01b: native file confinement for agent-directed paths.
//!
//! PRD 21.1 requires that a path an agent supplies cannot escape the
//! workspace, that the check is race-aware rather than string-based, and
//! that containment failure fails closed. PRD 21.2 adds the protected
//! names: `.git`, `.fluzo` and `.fluzodrive` are not agent-writable.
//!
//! The jail holds an anchored descriptor for the workspace root and
//! resolves every agent-supplied relative path against that descriptor,
//! one component at a time, with `O_NOFOLLOW` on every step. A string
//! prefix check would be wrong here for the usual reason: between the
//! check and the open another process can replace a component with a
//! symlink, and the open then follows it. Opening each component
//! relative to a descriptor we already hold removes the window, because
//! the descriptor cannot be redirected by a later rename.
//!
//! What this is not: an OS sandbox. It confines paths this jail is used
//! for. A process that opens files by absolute path directly is outside
//! its reach, and nothing here claims otherwise.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// Names an agent may not read through, write to, or rename over, at any
/// depth. `.git` carries hooks that execute; `.fluzo` is operator
/// configuration; `.fluzodrive` is durable state and the writer lock.
pub const PROTECTED_NAMES: &[&str] = &[".git", ".fluzo", ".fluzodrive"];

/// Confinement failures. Each is a refusal that dispatched nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfinementError {
    /// The supplied path is not a well-formed workspace-relative path:
    /// absolute, empty, containing `.`, `..`, an empty component or a
    /// NUL byte.
    UnsafePath { reason: String },
    /// A component of the path is protected. Reported with the offending
    /// name so the caller can say which one without re-parsing.
    Protected { name: String },
    /// The target does not exist inside the jail.
    NotFound,
    /// Resolution left the jail, or the target is a symlink. A symlink
    /// is treated as an escape whether it points inside or outside:
    /// following it would mean trusting a link another process can
    /// replace, and the agent did not ask to create links.
    Escape,
    /// A create was requested and something is already there.
    Exists,
    /// A directory was requested and the target is not one, or a
    /// directory operation met a non-directory component.
    NotADirectory,
    /// The target is not a regular file: a device node, FIFO, socket or
    /// similar. These are refused because opening one is not a file
    /// operation at all. A FIFO opened for reading blocks, a FIFO
    /// opened for writing can deliver bytes to an unrelated reader, and
    /// a device node writes wherever its major/minor pair points.
    NotARegularFile,
    /// The target has more than one hard link. Writing through it would
    /// change every other name for the same inode, including names
    /// outside the workspace, so the write is refused rather than
    /// silently shared.
    SharedLink { links: u64 },
    /// Any other I/O failure. Kept distinct so a real disk error is not
    /// reported as a confinement decision.
    Io,
}

impl std::fmt::Display for ConfinementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafePath { reason } => {
                write!(formatter, "unsafe workspace-relative path: {reason}")
            }
            Self::Protected { name } => write!(formatter, "{name} is protected from agent writes"),
            Self::NotFound => formatter.write_str("target not found in the workspace"),
            Self::Escape => formatter.write_str("path escapes the workspace or is a symlink"),
            Self::Exists => formatter.write_str("target already exists"),
            Self::NotADirectory => formatter.write_str("path component is not a directory"),
            Self::NotARegularFile => {
                formatter.write_str("target is not a regular file (device, FIFO or socket)")
            }
            Self::SharedLink { links } => write!(
                formatter,
                "target has {links} hard links; writing through it would change names outside the workspace"
            ),
            Self::Io => formatter.write_str("filesystem I/O failure inside the workspace"),
        }
    }
}

/// Splits an agent-supplied path into components, rejecting anything that
/// is not a plain relative path.
///
/// This is the first gate and it is deliberately strict: a path that
/// needs clever interpretation is refused rather than interpreted.
fn split_relative(relative: &str) -> Result<Vec<&str>, ConfinementError> {
    if relative.is_empty() {
        return Err(ConfinementError::UnsafePath {
            reason: "empty path".to_owned(),
        });
    }
    if relative.contains('\0') {
        return Err(ConfinementError::UnsafePath {
            reason: "path contains a NUL byte".to_owned(),
        });
    }
    if relative.starts_with('/') {
        return Err(ConfinementError::UnsafePath {
            reason: "absolute path".to_owned(),
        });
    }
    let mut parts = Vec::new();
    for part in relative.split('/') {
        if part.is_empty() {
            return Err(ConfinementError::UnsafePath {
                reason: "empty path component".to_owned(),
            });
        }
        if part == "." || part == ".." {
            return Err(ConfinementError::UnsafePath {
                reason: format!("path traversal component {part:?}"),
            });
        }
        if PROTECTED_NAMES.contains(&part) {
            return Err(ConfinementError::Protected {
                name: part.to_owned(),
            });
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err(ConfinementError::UnsafePath {
            reason: "no path components".to_owned(),
        });
    }
    Ok(parts)
}

/// A workspace jail. Holds the anchored root descriptor; every
/// operation resolves against it.
pub struct WorkspaceJail {
    root: File,
}

/// `std::fs::File`'s own `Debug` prints the path it was opened with.
/// PRD 21.1 says the workspace path must not end up in a trace by
/// default, so every type here that holds a descriptor writes its own
/// `Debug` and reports the descriptor number instead.
impl std::fmt::Debug for WorkspaceJail {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceJail")
            .field("root_fd", &self.root.as_raw_fd())
            .finish()
    }
}

impl WorkspaceJail {
    /// Opens a jail rooted at `root`, which must be an existing
    /// directory.
    ///
    /// The root is resolved and then re-opened component by component
    /// with `O_NOFOLLOW`, so the held descriptor is the directory the
    /// caller meant even if the path was rewritten meanwhile.
    pub fn open(root: &Path) -> Result<Self, ConfinementError> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err(ConfinementError::UnsafePath {
                reason: "confinement is implemented for Linux x86_64 only".to_owned(),
            });
        }
        let resolved = root
            .canonicalize()
            .map_err(|_| ConfinementError::NotFound)?;
        if !resolved.is_absolute()
            || resolved
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(ConfinementError::UnsafePath {
                reason: "resolved root is not a plain absolute path".to_owned(),
            });
        }
        let mut directory = File::open("/").map_err(|_| ConfinementError::Io)?;
        for component in resolved.components() {
            if let Component::Normal(name) = component {
                let path =
                    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
                directory = OpenOptions::new()
                    .read(true)
                    .custom_flags(crate::store::ANCHOR_FLAGS)
                    .open(path)
                    .map_err(|_| ConfinementError::Escape)?;
            }
        }
        Ok(Self { root: directory })
    }

    /// Resolves every component but the last, returning the descriptor
    /// for the containing directory and the final name.
    ///
    /// Each intermediate component is opened with `O_NOFOLLOW`, so a
    /// symlink anywhere along the path is an escape rather than a
    /// shortcut.
    fn resolve_parent(&self, parts: &[&str]) -> Result<(File, String), ConfinementError> {
        let mut directory = self.root.try_clone().map_err(|_| ConfinementError::Io)?;
        for part in &parts[..parts.len() - 1] {
            let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(part);
            directory = match OpenOptions::new()
                .read(true)
                .custom_flags(crate::store::ANCHOR_FLAGS)
                .open(path.clone())
            {
                Ok(opened) => opened,
                Err(error) => return Err(classify_component_error(error, &path)),
            };
        }
        Ok((directory, parts[parts.len() - 1].to_owned()))
    }

    /// Opens a directory inside the jail by anchored parent + name.
    fn open_directory(&self, parent: &File, name: &str) -> Result<File, ConfinementError> {
        let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
        OpenOptions::new()
            .read(true)
            .custom_flags(crate::store::ANCHOR_FLAGS)
            .open(&path)
            .map_err(|error| classify_component_error(error, &path))
    }

    /// Opens an existing file for reading.
    pub fn open_read(&self, relative: &str) -> Result<ConfinedFile, ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        let file = open_in(&parent, &name, OpenOptions::new().read(true), false)?;
        Ok(ConfinedFile {
            file,
            name: relative.to_owned(),
        })
    }

    /// Opens an existing file for writing without truncating it.
    pub fn open_write(&self, relative: &str) -> Result<ConfinedFile, ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        let file = open_in(
            &parent,
            &name,
            OpenOptions::new().read(true).write(true),
            false,
        )?;
        Ok(ConfinedFile {
            file,
            name: relative.to_owned(),
        })
    }

    /// Creates a new file, refusing if anything already exists.
    pub fn create(&self, relative: &str) -> Result<ConfinedFile, ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        let file = open_in(
            &parent,
            &name,
            OpenOptions::new().read(true).write(true).create_new(true),
            true,
        )?;
        Ok(ConfinedFile {
            file,
            name: relative.to_owned(),
        })
    }

    /// Creates a directory inside the jail.
    pub fn create_dir(&self, relative: &str) -> Result<ConfinedDir, ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        rustix::fs::mkdirat(
            &parent,
            name.as_str(),
            rustix::fs::Mode::from_bits_truncate(0o755),
        )
        .map_err(|error| classify(error.into(), true))?;
        let dir = self.open_directory(&parent, &name)?;
        Ok(ConfinedDir {
            dir,
            name: relative.to_owned(),
        })
    }

    /// Removes a file. A directory is not removed by this call.
    pub fn remove_file(&self, relative: &str) -> Result<(), ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        // Refuse to unlink something we have not confirmed is a regular
        // file: unlinkat with no flags cannot remove a directory, so a
        // directory here is a wrong-path mistake rather than a delete.
        let existing = open_in(&parent, &name, OpenOptions::new().read(true), false)?;
        let metadata = existing.metadata().map_err(|_| ConfinementError::Io)?;
        if !metadata.is_file() {
            return Err(ConfinementError::NotADirectory);
        }
        rustix::fs::unlinkat(&parent, name.as_str(), rustix::fs::AtFlags::empty())
            .map_err(|_| ConfinementError::NotFound)
    }

    /// Removes an empty directory.
    pub fn remove_dir(&self, relative: &str) -> Result<(), ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        rustix::fs::unlinkat(&parent, name.as_str(), rustix::fs::AtFlags::REMOVEDIR)
            .map_err(|_| ConfinementError::NotFound)
    }

    /// Renames within the jail. Both endpoints are resolved against the
    /// jail, so the destination cannot be outside it and cannot be a
    /// protected name.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), ConfinementError> {
        let from_parts = split_relative(from)?;
        let to_parts = split_relative(to)?;
        let (old_parent, old_name) = self.resolve_parent(&from_parts)?;
        let (new_parent, new_name) = self.resolve_parent(&to_parts)?;
        // Refuse to overwrite a symlink at the destination: renameat
        // would replace the link itself, which could be someone else's
        // pointer.
        let candidate =
            PathBuf::from(format!("/proc/self/fd/{}", new_parent.as_raw_fd())).join(&new_name);
        if std::fs::symlink_metadata(&candidate)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(ConfinementError::Escape);
        }
        rustix::fs::renameat(
            &old_parent,
            old_name.as_str(),
            &new_parent,
            new_name.as_str(),
        )
        .map_err(|_| ConfinementError::NotFound)
    }

    /// Lists the entries of a directory inside the jail.
    ///
    /// `.` names the jail root itself; every other path must be a plain
    /// relative path.
    pub fn read_dir(&self, relative: &str) -> Result<Vec<DirEntry>, ConfinementError> {
        if relative == "." {
            return self.list(&self.root);
        }
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        let directory = self.open_directory(&parent, &name)?;
        self.list(&directory)
    }

    fn list(&self, directory: &File) -> Result<Vec<DirEntry>, ConfinementError> {
        let mut entries = Vec::new();
        let mut dir =
            rustix::fs::Dir::read_from(directory).map_err(|_| ConfinementError::NotADirectory)?;
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(|_| ConfinementError::Io)?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "." || name == ".." {
                continue;
            }
            entries.push(DirEntry {
                name,
                kind: EntryKind::from(entry.file_type()),
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    /// Reports whether a path exists inside the jail, without following a
    /// symlink at the final component.
    pub fn symlink_metadata(&self, relative: &str) -> Result<Option<Metadata>, ConfinementError> {
        let parts = split_relative(relative)?;
        let (parent, name) = self.resolve_parent(&parts)?;
        let candidate = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(&name);
        match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => Ok(Some(Metadata {
                is_file: metadata.is_file(),
                is_dir: metadata.is_dir(),
                is_symlink: metadata.file_type().is_symlink(),
                len: metadata.len(),
            })),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(ConfinementError::Io),
        }
    }
}

fn open_in(
    parent: &File,
    name: &str,
    options: &OpenOptions,
    creating: bool,
) -> Result<File, ConfinementError> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
    let mut options = options.clone();
    // O_NOFOLLOW on the final component is the whole point: a symlink
    // here fails the open instead of being followed to somewhere the
    // agent did not name.
    options.custom_flags(crate::store::NOFOLLOW_CLOEXEC);
    if creating {
        options.mode(0o644);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) => {
            // A socket or a FIFO can fail at open, before there is a
            // descriptor to inspect, with a generic "not supported" or
            // "no such device or address". Refine the label so the
            // operator sees what was actually there. The refusal is
            // already decided by the open; this only names it.
            return Err(refine_unnamed_error(classify(error, creating), &path));
        }
    };
    verify_regular_and_unshared(&file)?;
    Ok(file)
}

/// Gives a generic I/O refusal a specific name when the path itself
/// explains it. Only ever called on a decision that already refused, so
/// a race on this stat cannot turn a refusal into a success.
fn refine_unnamed_error(decision: ConfinementError, anchored: &Path) -> ConfinementError {
    if decision != ConfinementError::Io {
        return decision;
    }
    match std::fs::symlink_metadata(anchored) {
        Ok(metadata) if !metadata.is_file() && !metadata.is_dir() => {
            ConfinementError::NotARegularFile
        }
        _ => decision,
    }
}

/// Checks the opened descriptor is a regular file with exactly one link.
///
/// This runs after the open, on the descriptor that will actually be
/// written through, so it describes the thing being modified rather
/// than whatever the name pointed at a moment ago. `O_NOFOLLOW` already
/// excluded symlinks; this excludes the rest of the inode types and
/// hard links, which `O_NOFOLLOW` says nothing about.
fn verify_regular_and_unshared(file: &File) -> Result<(), ConfinementError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(|_| ConfinementError::Io)?;
    if !metadata.is_file() {
        return Err(ConfinementError::NotARegularFile);
    }
    if metadata.nlink() != 1 {
        return Err(ConfinementError::SharedLink {
            links: metadata.nlink(),
        });
    }
    Ok(())
}

/// Maps an error from opening a *component* onto a confinement decision.
///
/// Linux reports `ENOTDIR` both for a component that is a plain
/// non-directory and for a symlink met with `O_NOFOLLOW | O_DIRECTORY`.
/// The refusal is the same either way and is already enforced by the
/// open flags; the extra `symlink_metadata` call below only chooses the
/// label, so an operator sees "escape attempt" rather than "not a
/// directory" when a link was involved. It is not the check that
/// protects anything, so a race on it cannot turn a refusal into a
/// success.
fn classify_component_error(error: std::io::Error, anchored: &Path) -> ConfinementError {
    if error.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error()) {
        return match std::fs::symlink_metadata(anchored) {
            Ok(metadata) if metadata.file_type().is_symlink() => ConfinementError::Escape,
            _ => ConfinementError::NotADirectory,
        };
    }
    match error.kind() {
        std::io::ErrorKind::NotFound => ConfinementError::NotFound,
        _ if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            ConfinementError::Escape
        }
        _ => ConfinementError::Io,
    }
}

/// Maps an open error onto a confinement decision. `creating` changes
/// what `AlreadyExists` means and keeps a symlink refusal from being
/// reported as "not found".
fn classify(error: std::io::Error, creating: bool) -> ConfinementError {
    match error.kind() {
        std::io::ErrorKind::NotFound => ConfinementError::NotFound,
        std::io::ErrorKind::AlreadyExists if creating => ConfinementError::Exists,
        // ELOOP from O_NOFOLLOW on a symlink.
        std::io::ErrorKind::InvalidInput => ConfinementError::Escape,
        _ if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) => {
            ConfinementError::Escape
        }
        _ if error.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error()) => {
            ConfinementError::NotADirectory
        }
        _ if error.raw_os_error() == Some(rustix::io::Errno::ISDIR.raw_os_error()) => {
            ConfinementError::NotADirectory
        }
        _ => ConfinementError::Io,
    }
}

/// One entry of a jail directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl EntryKind {
    fn from(kind: rustix::fs::FileType) -> Self {
        match kind {
            rustix::fs::FileType::Directory => EntryKind::Directory,
            rustix::fs::FileType::Symlink => EntryKind::Symlink,
            rustix::fs::FileType::RegularFile => EntryKind::File,
            _ => EntryKind::Other,
        }
    }
}

/// What `symlink_metadata` reported about a jail path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub len: u64,
}

/// A file opened inside the jail. The name it carries is the
/// workspace-relative name it was opened with; no absolute path is
/// retained, so a diagnostic printed from this value cannot leak the
/// workspace location (PRD 21.1).
pub struct ConfinedFile {
    file: File,
    name: String,
}

impl std::fmt::Debug for ConfinedFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfinedFile")
            .field("name", &self.name)
            .field("fd", &self.file.as_raw_fd())
            .finish()
    }
}

impl ConfinedFile {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), ConfinementError> {
        use std::io::Write;
        self.file.write_all(bytes).map_err(|_| ConfinementError::Io)
    }

    /// Replaces the whole contents: rewind, truncate, then write.
    ///
    /// A tool that writes a complete file must not leave the tail of the
    /// previous contents behind, which a plain `write_all` on a
    /// read/write handle does. Both the truncation and the write go through
    /// the descriptor the jail opened, so neither can be redirected onto a
    /// different inode after the fact.
    pub fn replace_all(&mut self, bytes: &[u8]) -> Result<(), ConfinementError> {
        use std::io::{Seek, SeekFrom, Write};
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| ConfinementError::Io)?;
        self.file.set_len(0).map_err(|_| ConfinementError::Io)?;
        self.file.write_all(bytes).map_err(|_| ConfinementError::Io)
    }

    pub fn read_to_string(&mut self) -> Result<String, ConfinementError> {
        use std::io::Read;
        let mut text = String::new();
        self.file
            .read_to_string(&mut text)
            .map_err(|_| ConfinementError::Io)?;
        Ok(text)
    }

    pub fn sync_all(&self) -> Result<(), ConfinementError> {
        self.file.sync_all().map_err(|_| ConfinementError::Io)
    }

    pub fn len(&self) -> Result<u64, ConfinementError> {
        self.file
            .metadata()
            .map(|m| m.len())
            .map_err(|_| ConfinementError::Io)
    }

    pub fn is_empty(&self) -> Result<bool, ConfinementError> {
        Ok(self.len()? == 0)
    }
}

/// A directory opened inside the jail.
pub struct ConfinedDir {
    dir: File,
    name: String,
}

impl std::fmt::Debug for ConfinedDir {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfinedDir")
            .field("name", &self.name)
            .field("fd", &self.dir.as_raw_fd())
            .finish()
    }
}

impl ConfinedDir {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Flushes the directory entry itself. A create that is not synced
    /// here can survive the file's own sync and still be absent after a
    /// crash, because the name lives in the directory.
    pub fn sync_all(&self) -> Result<(), ConfinementError> {
        self.dir.sync_all().map_err(|_| ConfinementError::Io)
    }
}

#[cfg(test)]
#[path = "confinement_tests.rs"]
mod tests;
