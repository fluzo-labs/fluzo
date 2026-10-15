//! DAT-02a artifact layer: bounded capture, durability-separated
//! finalization and reference integrity over the DAT-01 store.
//!
//! Contract (PRD 22.2/44.5, Architecture A03, issue #72): artifact bytes
//! live under the managed root `.fluzodrive/artifacts/` and are written
//! only through this store-owned, drive-anchored writer. Finalization
//! (complete, size recorded, immutable content file) is a separate
//! durability step that strictly precedes the SQL reference commit; the
//! reference row then commits atomically with the owning state update and
//! journal event in one `BEGIN IMMEDIATE` unit. Capture is bounded by a
//! finite per-output limit, and truncation, redaction or omission are
//! always marked in the record — never silent. Verification compares the
//! persisted record against the content file and reports corruption
//! without ever repairing or re-finalizing.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use sqlx::Connection;

use crate::store::{EventRecord, NOFOLLOW_CLOEXEC, StateUpdate, Store, StoreError};

/// Managed artifact root inside `.fluzodrive/`.
pub const ARTIFACTS_DIRECTORY: &str = "artifacts";
/// Finite per-output capture limit. Larger inputs are stored as the
/// leading prefix and explicitly marked `Truncated`.
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;
const REDACTED_MARKER: &[u8] = b"[REDACTED]";
const MAX_ID_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureFlag {
    Truncated,
    Redacted,
    Omitted,
}

impl CaptureFlag {
    fn as_str(self) -> &'static str {
        match self {
            CaptureFlag::Truncated => "truncated",
            CaptureFlag::Redacted => "redacted",
            CaptureFlag::Omitted => "omitted",
        }
    }
}

fn encode_flags(flags: &[CaptureFlag]) -> String {
    // Canonical order keeps the persisted form stable.
    let mut names: Vec<&str> = flags.iter().map(|flag| flag.as_str()).collect();
    names.sort_unstable();
    names.join(",")
}

fn decode_flags(raw: &str) -> Vec<CaptureFlag> {
    raw.split(',')
        .filter(|name| !name.is_empty())
        .map(|name| match name {
            "truncated" => CaptureFlag::Truncated,
            "redacted" => CaptureFlag::Redacted,
            _ => CaptureFlag::Omitted,
        })
        .collect()
}

/// Identity, owning state row and capture provenance for one artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSpec {
    pub id: String,
    pub state_id: String,
    pub kind: String,
    pub provenance: String,
}

/// Fixture-driven producer input. `secrets` are literal values supplied
/// by the caller's redaction layer; they are removed from content and
/// record fields before anything is persisted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRequest {
    pub spec: ArtifactSpec,
    pub content: Vec<u8>,
    pub secrets: Vec<String>,
    pub omitted: bool,
}

/// Persisted artifact metadata. Returned by finalization and by record
/// reads; `content_hash` is the SHA-256 of the stored bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactRecord {
    pub id: String,
    pub state_id: String,
    pub kind: String,
    pub provenance: String,
    pub stored_bytes: i64,
    pub content_hash: String,
    pub flags: Vec<CaptureFlag>,
}

/// Result of comparing a committed record against the content file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactStatus {
    Missing,
    Verified {
        stored_bytes: i64,
        content_hash: String,
    },
    Corrupt {
        reason: String,
    },
}

/// A file under the managed artifact root with no committed reference:
/// detectable and reclaimable (reclamation itself is DAT-02d purge scope).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrphanFile {
    pub name: String,
    pub size: u64,
}

fn is_safe_artifact_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Replaces every occurrence of each non-empty secret with the redaction
/// marker. Returns the redacted bytes and whether anything changed.
fn redact_bytes(content: &[u8], secrets: &[String]) -> (Vec<u8>, bool) {
    let mut out = content.to_vec();
    let mut changed = false;
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        let needle = secret.as_bytes();
        let mut replaced = Vec::with_capacity(out.len());
        let mut cursor = 0usize;
        while cursor < out.len() {
            if out[cursor..].starts_with(needle) {
                replaced.extend_from_slice(REDACTED_MARKER);
                cursor += needle.len();
                changed = true;
            } else {
                replaced.push(out[cursor]);
                cursor += 1;
            }
        }
        out = replaced;
    }
    (out, changed)
}

impl Store {
    /// Durability step 1: validate, redact, bound and finalize the
    /// content file. The finalized artifact is immutable; nothing is
    /// written to SQL here. Returns the record to be referenced.
    pub async fn finalize_artifact(
        &mut self,
        request: &CaptureRequest,
    ) -> Result<ArtifactRecord, StoreError> {
        if !is_safe_artifact_id(&request.spec.id) || !is_safe_artifact_id(&request.spec.state_id) {
            return Err(StoreError::UnsupportedPath);
        }
        self.verify_drive()?;
        if self.artifact_record_raw(&request.spec.id).await?.is_some() {
            return Err(StoreError::Conflict);
        }
        let artifacts = self.anchor_artifacts(true)?;
        let final_path = Self::anchored_path(&artifacts, &request.spec.id);
        let staging_path =
            Self::anchored_path(&artifacts, &format!(".staging-{}", request.spec.id));
        if fs::symlink_metadata(&final_path).is_ok() || fs::symlink_metadata(&staging_path).is_ok()
        {
            return Err(StoreError::Conflict);
        }

        let (kind, kind_redacted) = redact_str(&request.spec.kind, &request.secrets);
        let (provenance, prov_redacted) = redact_str(&request.spec.provenance, &request.secrets);
        let mut flags = Vec::new();
        let (stored_bytes, content_hash) = if request.omitted {
            if kind_redacted || prov_redacted {
                flags.push(CaptureFlag::Redacted);
            }
            flags.push(CaptureFlag::Omitted);
            (0i64, String::new())
        } else {
            let (mut bytes, content_redacted) = redact_bytes(&request.content, &request.secrets);
            if content_redacted || kind_redacted || prov_redacted {
                flags.push(CaptureFlag::Redacted);
            }
            if bytes.len() > MAX_ARTIFACT_BYTES {
                bytes.truncate(MAX_ARTIFACT_BYTES);
                flags.push(CaptureFlag::Truncated);
            }
            let hash = sha256_hex(&bytes);
            // DAT-02b: the content file is session data, so admission is
            // checked before any byte of it lands on disk.
            self.admit_session_bytes(bytes.len() as u64)?;
            finalize_file(&artifacts, &request.spec.id, &staging_path, &bytes)?;
            (bytes.len() as i64, hash)
        };
        Ok(ArtifactRecord {
            id: request.spec.id.clone(),
            state_id: request.spec.state_id.clone(),
            kind,
            provenance,
            stored_bytes,
            content_hash,
            flags,
        })
    }

    /// Durability step 2: commit the artifact reference together with the
    /// owning state updates and journal events in one `BEGIN IMMEDIATE`
    /// unit. The finalized content file is re-checked first: a reference
    /// can never commit without its finalized bytes present and matching.
    pub async fn commit_artifact_reference(
        &mut self,
        record: &ArtifactRecord,
        updates: &[StateUpdate],
        events: &[EventRecord],
    ) -> Result<(), StoreError> {
        self.verify_drive()?;
        self.admit_session_work(updates, events)?;
        if !record.flags.contains(&CaptureFlag::Omitted) {
            let artifacts = self.anchor_artifacts(false)?;
            let path = Self::anchored_path(&artifacts, &record.id);
            let bytes = match read_anchored(&path) {
                Ok(bytes) => bytes,
                Err(_) => return Err(StoreError::Conflict),
            };
            if bytes.len() as i64 != record.stored_bytes
                || sha256_hex(&bytes) != record.content_hash
            {
                return Err(StoreError::Corrupt);
            }
        }
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::store::map_error)?;
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
            .map_err(crate::store::map_error)?;
        }
        sqlx::query(
            "INSERT INTO artifact (id, state_id, kind, provenance, stored_bytes, content_hash, capture_flags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&record.id)
        .bind(&record.state_id)
        .bind(&record.kind)
        .bind(&record.provenance)
        .bind(record.stored_bytes)
        .bind(&record.content_hash)
        .bind(encode_flags(&record.flags))
        .execute(&mut *transaction)
        .await
        .map_err(crate::store::map_error)?;
        for event in events {
            sqlx::query("INSERT INTO event (state_id, kind, payload) VALUES (?1, ?2, ?3)")
                .bind(&event.state_id)
                .bind(&event.kind)
                .bind(&event.payload)
                .execute(&mut *transaction)
                .await
                .map_err(crate::store::map_error)?;
        }
        transaction.commit().await.map_err(crate::store::map_error)
    }

    /// Persisted record for `id`, without touching the content file.
    pub async fn artifact_record(
        &mut self,
        id: &str,
    ) -> Result<Option<ArtifactRecord>, StoreError> {
        self.artifact_record_raw(id).await
    }

    /// Compare the committed record with the content file. Corruption is
    /// reported, never repaired, and never re-finalized.
    pub async fn verify_artifact(&mut self, id: &str) -> Result<ArtifactStatus, StoreError> {
        let Some(record) = self.artifact_record_raw(id).await? else {
            return Ok(ArtifactStatus::Missing);
        };
        if record.flags.contains(&CaptureFlag::Omitted) {
            return Ok(ArtifactStatus::Verified {
                stored_bytes: 0,
                content_hash: String::new(),
            });
        }
        let artifacts = self.anchor_artifacts(false)?;
        let path = Self::anchored_path(&artifacts, &record.id);
        let bytes = match read_anchored(&path) {
            Ok(bytes) => bytes,
            Err(reason) => return Ok(ArtifactStatus::Corrupt { reason }),
        };
        if bytes.len() as i64 != record.stored_bytes {
            return Ok(ArtifactStatus::Corrupt {
                reason: "stored size mismatch".into(),
            });
        }
        let hash = sha256_hex(&bytes);
        if hash != record.content_hash {
            return Ok(ArtifactStatus::Corrupt {
                reason: "content hash mismatch".into(),
            });
        }
        Ok(ArtifactStatus::Verified {
            stored_bytes: record.stored_bytes,
            content_hash: hash,
        })
    }

    /// Files under the managed artifact root with no committed reference
    /// (finalized-but-uncommitted content and leftover staging files).
    /// Detection only: reclamation is DAT-02d purge scope.
    pub async fn detect_orphaned_artifacts(&mut self) -> Result<Vec<OrphanFile>, StoreError> {
        let artifacts = self.anchor_artifacts(false)?;
        let mut committed = std::collections::HashSet::new();
        let rows = sqlx::query("SELECT id FROM artifact")
            .fetch_all(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        use sqlx::Row;
        for row in rows {
            let id: String = row.try_get(0).map_err(crate::store::map_error)?;
            committed.insert(id);
        }
        let mut orphans = Vec::new();
        let dir_path = PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&artifacts)
        ));
        let entries = fs::read_dir(&dir_path).map_err(|_| StoreError::Inaccessible)?;
        for entry in entries {
            let entry = entry.map_err(|_| StoreError::Inaccessible)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| StoreError::Io)?;
            if !metadata.is_file() {
                continue;
            }
            let base = name.strip_prefix(".staging-").unwrap_or(&name);
            if !committed.contains(base) {
                orphans.push(OrphanFile {
                    name,
                    size: metadata.len(),
                });
            }
        }
        orphans.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(orphans)
    }

    /// Verify-then-read through the anchored root. Corrupt content is an
    /// error and is never returned as if valid.
    pub async fn read_artifact_content(&mut self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self.verify_artifact(id).await? {
            ArtifactStatus::Missing => Ok(None),
            ArtifactStatus::Corrupt { .. } => Err(StoreError::Corrupt),
            ArtifactStatus::Verified { .. } => {
                let record = self
                    .artifact_record_raw(id)
                    .await?
                    .ok_or(StoreError::Corrupt)?;
                if record.flags.contains(&CaptureFlag::Omitted) {
                    return Ok(Some(Vec::new()));
                }
                let artifacts = self.anchor_artifacts(false)?;
                let path = Self::anchored_path(&artifacts, id);
                read_anchored(&path)
                    .map(Some)
                    .map_err(|_| StoreError::Corrupt)
            }
        }
    }

    pub(crate) fn anchored_path(dir: &File, name: &str) -> PathBuf {
        PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(dir)
        ))
        .join(name)
    }

    fn anchor_artifacts(&self, create: bool) -> Result<File, StoreError> {
        self.anchor_class(ARTIFACTS_DIRECTORY, create)
    }

    async fn artifact_record_raw(
        &mut self,
        id: &str,
    ) -> Result<Option<ArtifactRecord>, StoreError> {
        let row = sqlx::query(
            "SELECT id, state_id, kind, provenance, stored_bytes, content_hash, capture_flags
             FROM artifact WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&mut self.connection)
        .await
        .map_err(crate::store::map_error)?;
        use sqlx::Row;
        Ok(row.map(|row| ArtifactRecord {
            id: row.try_get(0).unwrap_or_default(),
            state_id: row.try_get(1).unwrap_or_default(),
            kind: row.try_get(2).unwrap_or_default(),
            provenance: row.try_get(3).unwrap_or_default(),
            stored_bytes: row.try_get(4).unwrap_or_default(),
            content_hash: row.try_get(5).unwrap_or_default(),
            flags: decode_flags(&row.try_get::<String, _>(6).unwrap_or_default()),
        }))
    }
}

fn redact_str(value: &str, secrets: &[String]) -> (String, bool) {
    let (bytes, changed) = redact_bytes(value.as_bytes(), secrets);
    (String::from_utf8_lossy(&bytes).into_owned(), changed)
}

/// Write staging, fsync, rename into place, fsync the directory: the
/// content file is complete and immutable before any reference exists.
fn finalize_file(
    artifacts: &File,
    id: &str,
    staging: &Path,
    bytes: &[u8],
) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(NOFOLLOW_CLOEXEC)
            .open(staging)
            .map_err(|_| StoreError::Conflict)?;
        file.write_all(bytes).map_err(|_| {
            let _ = fs::remove_file(staging);
            StoreError::Io
        })?;
        file.sync_all().map_err(|_| {
            let _ = fs::remove_file(staging);
            StoreError::Io
        })?;
        drop(file);
        let final_path = Store::anchored_path(artifacts, id);
        fs::rename(staging, &final_path).map_err(|_| StoreError::Io)?;
        artifacts.sync_all().map_err(|_| StoreError::Io)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (artifacts, id, staging, bytes);
        Err(StoreError::UnsupportedPath)
    }
}

/// Read through O_NOFOLLOW; returns Err(reason) for missing, symlinked
/// or unreadable content without following links.
fn read_anchored(path: &Path) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(NOFOLLOW_CLOEXEC)
            .open(path)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => "content file missing".to_string(),
                _ => "content file unreadable (symlink or unsafe)".to_string(),
            })?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)
            .map_err(|_| "content read failed".to_string())?;
        Ok(bytes)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("unsupported platform".to_string())
    }
}
