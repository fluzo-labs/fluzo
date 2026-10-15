//! OBS-01 bounded local diagnostic archive: versioned JSONL segments
//! under `.fluzodrive/telemetry/`.
//!
//! Records arrive pre-serialized and already redacted by the diagnostic
//! hub; this layer only performs bounded batching, segment rotation and
//! durable writes off the producer path. Rotation never expires or
//! overwrites old records. A reader reports incomplete trailing records
//! after a crash instead of presenting them as committed, and never
//! deletes prior data silently.

use std::fs::{self, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const TELEMETRY_DIRECTORY: &str = "telemetry";
pub const SEGMENT_PREFIX: &str = "segment-";
pub const SEGMENT_SUFFIX: &str = "jsonl";

#[derive(Debug)]
pub enum ArchiveError {
    /// The archive root is missing, symlinked or otherwise unsafe.
    UnsafeRoot,
    Io(String),
}

/// Writes JSONL segments with rotation by byte bound. Owned by one
/// worker; never called from the producer or TUI input path.
pub struct JsonlArchive {
    directory: PathBuf,
    segment_index: u64,
    segment_bytes: u64,
    rotate_bytes: u64,
}

impl JsonlArchive {
    /// Opens (creating when absent) `.fluzodrive/telemetry/` under the
    /// workspace root. A symlinked or non-directory root fails closed.
    pub fn open(workspace_root: &Path) -> Result<Self, ArchiveError> {
        let directory = workspace_root
            .join(crate::store::DRIVE_DIRECTORY)
            .join(TELEMETRY_DIRECTORY);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ArchiveError::UnsafeRoot);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&directory).map_err(|e| ArchiveError::Io(e.to_string()))?;
                #[cfg(unix)]
                fs::set_permissions(
                    &directory,
                    std::os::unix::fs::PermissionsExt::from_mode(0o700),
                )
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            }
            Err(error) => return Err(ArchiveError::Io(error.to_string())),
        }
        let segment_index = Self::latest_segment_index(&directory)?;
        let segment_bytes = fs::metadata(Self::segment_path(&directory, segment_index))
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(Self {
            directory,
            segment_index,
            segment_bytes,
            rotate_bytes: 1024 * 1024,
        })
    }

    pub fn with_rotation(mut self, rotate_bytes: u64) -> Self {
        self.rotate_bytes = rotate_bytes.max(1);
        self
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn segment_path(directory: &Path, index: u64) -> PathBuf {
        directory.join(format!("{SEGMENT_PREFIX}{index:06}.{SEGMENT_SUFFIX}"))
    }

    /// Parses a segment file name into its index, `None` for anything else.
    fn segment_index_of(name: &str) -> Option<u64> {
        let rest = name.strip_prefix(SEGMENT_PREFIX)?;
        let digits = rest.strip_suffix(&format!(".{SEGMENT_SUFFIX}"))?;
        digits.parse::<u64>().ok()
    }

    fn latest_segment_index(directory: &Path) -> Result<u64, ArchiveError> {
        let mut latest = 0u64;
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(ArchiveError::Io(error.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| ArchiveError::Io(e.to_string()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(index) = Self::segment_index_of(&name) {
                latest = latest.max(index);
            }
        }
        Ok(latest)
    }

    /// Appends a batch of complete JSON lines, fsyncs, and rotates to a
    /// new segment when the byte bound is exceeded. Rotation never
    /// rewrites or removes existing segments.
    pub fn append_batch(&mut self, lines: &[String]) -> Result<(), ArchiveError> {
        if lines.is_empty() {
            return Ok(());
        }
        let path = Self::segment_path(&self.directory, self.segment_index);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| ArchiveError::Io(e.to_string()))?;
        let mut written = 0u64;
        for line in lines {
            if line.contains('\n') {
                return Err(ArchiveError::Io(
                    "record line must not contain newline".into(),
                ));
            }
            let payload = format!("{line}\n");
            file.write_all(payload.as_bytes())
                .map_err(|e| ArchiveError::Io(e.to_string()))?;
            written += payload.len() as u64;
        }
        file.sync_all()
            .map_err(|e| ArchiveError::Io(e.to_string()))?;
        self.segment_bytes += written;
        if self.segment_bytes >= self.rotate_bytes {
            self.segment_index += 1;
            self.segment_bytes = 0;
        }
        Ok(())
    }
}

/// One line read back from the archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveLine {
    /// A complete newline-terminated record (kept as raw JSON text).
    Complete(String),
    /// A trailing partial record found after a crash. Reported, never
    /// presented as committed and never silently deleted.
    IncompleteTrailing { bytes: usize },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArchiveScan {
    pub segments: usize,
    pub lines: Vec<ArchiveLine>,
}

/// Reads all segments in order. Only the final trailing partial record
/// of each segment is reported as incomplete; anything else that fails
/// to parse stays `Complete` for the caller to classify.
pub fn read_archive(directory: &Path) -> Result<ArchiveScan, ArchiveError> {
    let mut scan = ArchiveScan::default();
    let mut indexes = Vec::new();
    let entries = fs::read_dir(directory).map_err(|e| ArchiveError::Io(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| ArchiveError::Io(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(index) = JsonlArchive::segment_index_of(&name) {
            indexes.push(index);
        }
    }
    indexes.sort_unstable();
    scan.segments = indexes.len();
    for index in indexes {
        let bytes = fs::read(JsonlArchive::segment_path(directory, index))
            .map_err(|e| ArchiveError::Io(e.to_string()))?;
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            match bytes[cursor..].iter().position(|b| *b == b'\n') {
                Some(offset) => {
                    let line =
                        String::from_utf8_lossy(&bytes[cursor..cursor + offset]).into_owned();
                    scan.lines.push(ArchiveLine::Complete(line));
                    cursor += offset + 1;
                }
                None => {
                    scan.lines.push(ArchiveLine::IncompleteTrailing {
                        bytes: bytes.len() - cursor,
                    });
                    break;
                }
            }
        }
    }
    Ok(scan)
}
