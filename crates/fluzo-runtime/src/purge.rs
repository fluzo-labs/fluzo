//! DAT-02d: history purge with dry-run, unbypassable confirmation and
//! shared-reference protection.
//!
//! A *session* at this layer is a `state` row together with the `event`
//! rows and `artifact` rows that reference it. Everything the purge
//! surface reports comes from those durable records: listing never
//! deletes, never executes and never contacts a provider.
//!
//! **Shared-reference rule.** `artifact.state_id` is a single-owner
//! foreign key and DAT-02a rules out cross-session deduplication, so a
//! shared artifact cannot survive as a row of the session being purged:
//! removing that session row would violate the key. Sharing is therefore
//! analysed on the recorded `content_hash` -- the field DAT-02a persists
//! for exactly this -- and a shared artifact is preserved by moving its
//! ownership to a surviving holder instead of deleting it. The artifact
//! row and its content file both stay, the purged session stops owning
//! them, and no orphan is created. An artifact whose content no other
//! session holds is exclusive and is removed together with its file.
//!
//! Sharing is evaluated against the sessions *not* being purged, so
//! `--all` correctly classifies everything as exclusive rather than
//! preserving rows into a repository with no owners left.
//!
//! **Two-step durability.** Row removal commits first, content files are
//! removed after. Holding the write lock across filesystem deletion would
//! put an I/O wait inside the transaction, which the storage rules forbid.
//! A crash between the two leaves files with no committed row; those are
//! orphans, and `reclaim_orphan_artifact_files` finishes the job. That is
//! the same reclaim path DAT-02a points at for an interrupted capture.

use crate::artifact::ARTIFACTS_DIRECTORY;
use crate::store::{Store, StoreError};
use sqlx::{Connection as _, Row as _};
use std::fs;
use std::path::PathBuf;

/// How an actual deletion was confirmed.
///
/// Deliberately independent of any YOLO mode: the issue requires that
/// confirmation cannot be bypassed by it, so nothing here reads a
/// bypass flag. A caller in YOLO still has to supply `Prompted` or
/// `ExplicitYes`, and supplying `None` deletes nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Confirmation {
    /// The operator answered an interactive prompt naming this plan.
    Prompted,
    /// `--yes` was passed explicitly.
    ExplicitYes,
    /// Nothing was confirmed.
    None,
}

impl Confirmation {
    pub fn authorizes_deletion(self) -> bool {
        matches!(self, Self::Prompted | Self::ExplicitYes)
    }
}

/// An artifact that will be removed with its content file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgedArtifact {
    pub id: String,
    pub kind: String,
    pub bytes: i64,
}

/// An artifact kept because another session still holds the same content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservedArtifact {
    pub id: String,
    pub kind: String,
    pub bytes: i64,
    /// Sessions outside the purge set that hold the same `content_hash`.
    /// The artifact is re-owned by the first of them.
    pub held_by: Vec<String>,
}

/// The per-session half of a purge plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgeSessionPlan {
    pub id: String,
    pub kind: String,
    pub event_count: i64,
    pub event_bytes: i64,
    pub exclusive: Vec<PurgedArtifact>,
    pub preserved: Vec<PreservedArtifact>,
    /// Bytes this session's exclusive artifacts release.
    pub bytes_released: i64,
}

/// A complete, side-effect-free purge preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgePlan {
    /// Display name of the selected repository.
    pub repository: String,
    /// Absolute path of the `.fluzodrive/` directory the purge may touch.
    pub drive_path: String,
    pub sessions: Vec<PurgeSessionPlan>,
    pub total_bytes_released: i64,
}

impl PurgePlan {
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn preserved_count(&self) -> usize {
        self.sessions.iter().map(|s| s.preserved.len()).sum()
    }
}

/// Result of an executed purge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgeOutcome {
    pub plan: PurgePlan,
    /// Exclusive artifact files that could not be removed after the rows
    /// were committed. They are orphans and the retry path collects them.
    pub files_pending: Vec<String>,
    /// Bytes released by content files actually removed in this call.
    pub released_bytes: i64,
}

/// Report from finishing an interrupted purge.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct RemainderReport {
    pub reclaimed: Vec<String>,
    pub failed: Vec<String>,
    pub reclaimed_bytes: i64,
}

/// Why a purge was refused. Every variant means nothing was deleted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PurgeRefusal {
    /// The named session has no durable record.
    NoSuchSession(String),
    /// The session is attached and must be stopped or detached first.
    /// The named session is the one that blocks.
    SessionAttached(String),
    /// Nothing confirmed the deletion.
    ConfirmationRequired,
    /// No session matched the request.
    NothingToPurge,
}

impl std::fmt::Display for PurgeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSession(id) => write!(f, "no such session: {id}"),
            Self::SessionAttached(id) => {
                write!(f, "session {id} is attached; stop or detach it first")
            }
            Self::ConfirmationRequired => {
                write!(f, "deletion requires confirmation or an explicit --yes")
            }
            Self::NothingToPurge => write!(f, "no sessions selected to purge"),
        }
    }
}

/// Why a purge did not run, or failed while running.
///
/// The two arms stay distinct on purpose: a `Refused` means nothing was
/// deleted and the caller can act on the reason, while `Store` is a
/// storage failure at or after the point work was authorized.
#[derive(Debug)]
pub enum PurgeError {
    Refused(PurgeRefusal),
    Store(StoreError),
}

impl From<PurgeRefusal> for PurgeError {
    fn from(value: PurgeRefusal) -> Self {
        Self::Refused(value)
    }
}

impl From<StoreError> for PurgeError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl std::fmt::Display for PurgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "{refusal}"),
            Self::Store(error) => write!(f, "store failure: {error:?}"),
        }
    }
}

/// One row of the read-only history listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub kind: String,
    /// Payload bytes recorded for the session itself.
    pub payload_bytes: i64,
    pub event_count: i64,
    pub event_bytes: i64,
    pub artifact_count: i64,
    pub artifact_bytes: i64,
    pub attached: bool,
}

impl SessionSummary {
    pub fn total_bytes(&self) -> i64 {
        self.payload_bytes + self.event_bytes + self.artifact_bytes
    }
}

struct ArtifactRow {
    id: String,
    kind: String,
    stored_bytes: i64,
    content_hash: String,
}

// Deterministic mid-purge filesystem failure injection.
//
// A partial purge has to be exercised without racing the filesystem, so
// the removal step consults this set first. Test-only: no production
// path reads it, and an empty set is the normal case.
#[cfg(test)]
thread_local! {
    static PURGE_FILE_FAILURES: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

#[cfg(test)]
pub fn fail_artifact_removal_for_test(ids: &[&str]) {
    PURGE_FILE_FAILURES.with(|failures| {
        for id in ids {
            failures.borrow_mut().insert((*id).to_string());
        }
    });
}

#[cfg(test)]
pub fn clear_artifact_removal_failures_for_test() {
    PURGE_FILE_FAILURES.with(|failures| failures.borrow_mut().clear());
}

impl Store {
    /// Lists this repository's sessions from durable records only. The
    /// query is read-only: it never deletes, never executes and never
    /// contacts a provider.
    pub async fn history_summary(&mut self) -> Result<Vec<SessionSummary>, StoreError> {
        let states = sqlx::query("SELECT id, kind, length(payload) FROM state ORDER BY id")
            .fetch_all(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        let mut out = Vec::with_capacity(states.len());
        for row in states {
            let id: String = row.try_get(0).map_err(crate::store::map_error)?;
            let kind: String = row.try_get(1).map_err(crate::store::map_error)?;
            let payload_bytes: i64 = row.try_get(2).map_err(crate::store::map_error)?;
            let (event_count, event_bytes) = self.event_totals(&id).await?;
            let artifacts = self.artifact_rows_for(&id).await?;
            let artifact_bytes: i64 = artifacts.iter().map(|a| a.stored_bytes).sum();
            let attached = self.is_attached_raw(&id).await?;
            out.push(SessionSummary {
                id,
                kind,
                payload_bytes,
                event_count,
                event_bytes,
                artifact_count: artifacts.len() as i64,
                artifact_bytes,
                attached,
            });
        }
        Ok(out)
    }

    /// Builds a side-effect-free preview of purging `ids`.
    ///
    /// The plan reads current durable state and changes nothing. Callers
    /// that show a preview and then delete must let the delete recompute
    /// the plan rather than trusting an earlier one: reference state is
    /// only authoritative at deletion time.
    pub async fn plan_purge(&mut self, ids: &[String]) -> Result<PurgePlan, PurgeError> {
        let mut plan = PurgePlan {
            repository: self
                .drive_directory
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| "<repository unavailable>".into()),
            drive_path: self.drive_directory.to_string_lossy().into_owned(),
            sessions: Vec::new(),
            total_bytes_released: 0,
        };
        let purge_set: Vec<&str> = ids.iter().map(String::as_str).collect();
        for id in ids {
            let Some(kind) = self.state_kind(id).await? else {
                return Err(PurgeRefusal::NoSuchSession(id.clone()).into());
            };
            if self.is_attached_raw(id).await? {
                return Err(PurgeRefusal::SessionAttached(id.clone()).into());
            }
            let (event_count, event_bytes) = self.event_totals(id).await?;
            let artifacts = self.artifact_rows_for(id).await?;
            let mut exclusive = Vec::new();
            let mut preserved = Vec::new();
            for artifact in artifacts {
                // Omitted captures have no content file; they are removed
                // with the row and release nothing.
                if artifact.content_hash.is_empty() {
                    exclusive.push(PurgedArtifact {
                        id: artifact.id,
                        kind: artifact.kind,
                        bytes: 0,
                    });
                    continue;
                }
                let holders = self
                    .other_holders(&artifact.content_hash, &purge_set)
                    .await?;
                if holders.is_empty() {
                    exclusive.push(PurgedArtifact {
                        id: artifact.id,
                        kind: artifact.kind,
                        bytes: artifact.stored_bytes,
                    });
                } else {
                    preserved.push(PreservedArtifact {
                        id: artifact.id,
                        kind: artifact.kind,
                        bytes: artifact.stored_bytes,
                        held_by: holders,
                    });
                }
            }
            let bytes_released = exclusive.iter().map(|a| a.bytes).sum();
            plan.total_bytes_released += bytes_released;
            plan.sessions.push(PurgeSessionPlan {
                id: id.clone(),
                kind,
                event_count,
                event_bytes,
                exclusive,
                preserved,
                bytes_released,
            });
        }
        Ok(plan)
    }

    /// Executes a purge. Nothing is removed unless `confirmation`
    /// authorizes it, and a refusal deletes nothing at all.
    ///
    /// Row removal happens in one `BEGIN IMMEDIATE` unit per session and
    /// content files are removed afterwards, outside the transaction.
    /// Files that fail to remove are reported in `files_pending` and are
    /// collected by `reclaim_orphan_artifact_files`.
    pub async fn purge_sessions(
        &mut self,
        ids: &[String],
        confirmation: Confirmation,
    ) -> Result<PurgeOutcome, PurgeError> {
        if !confirmation.authorizes_deletion() {
            return Err(PurgeRefusal::ConfirmationRequired.into());
        }
        if ids.is_empty() {
            return Err(PurgeRefusal::NothingToPurge.into());
        }
        // Recomputed here, not carried over from any earlier preview.
        let plan = self.plan_purge(ids).await?;
        let mut files_pending = Vec::new();
        let mut released_bytes = 0i64;
        for session in &plan.sessions {
            self.delete_session_rows(session).await?;
            // Filesystem phase, outside the transaction.
            for artifact in &session.exclusive {
                match self.remove_artifact_file(&artifact.id) {
                    Ok(removed) => released_bytes += removed as i64,
                    Err(_) => files_pending.push(artifact.id.clone()),
                }
            }
        }
        Ok(PurgeOutcome {
            plan,
            files_pending,
            released_bytes,
        })
    }

    /// Finishes a purge that stopped after the rows were committed.
    ///
    /// Removes every artifact file with no committed reference row and
    /// leaves everything a row still describes untouched. Safe to repeat:
    /// once the files are gone there is nothing left to reclaim, and
    /// quota is recomputed from disk so a repeat releases nothing twice.
    pub async fn reclaim_orphan_artifact_files(&mut self) -> Result<RemainderReport, StoreError> {
        self.verify_drive()?;
        let rows = sqlx::query("SELECT id FROM artifact")
            .fetch_all(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        let mut referenced = std::collections::HashSet::new();
        for row in rows {
            let id: String = row.try_get(0).map_err(crate::store::map_error)?;
            referenced.insert(id);
        }
        let anchor = self.anchor_class(ARTIFACTS_DIRECTORY, false)?;
        let dir = PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&anchor)
        ));
        let mut report = RemainderReport::default();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return Ok(report),
        };
        let mut names = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else { continue };
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        for name in names {
            let path = Store::anchored_path(&anchor, &name);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            let base = name.strip_prefix(".staging-").unwrap_or(&name).to_string();
            if referenced.contains(&base) {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    report.reclaimed.push(name);
                    report.reclaimed_bytes += metadata.len() as i64;
                }
                Err(_) => report.failed.push(name),
            }
        }
        Ok(report)
    }

    /// Marks a session attached so purge refuses it. The marker is
    /// durable because the attaching process is not the later purging
    /// process.
    pub async fn attach_session(&mut self, id: &str, attached_at: i64) -> Result<(), StoreError> {
        if self.state_kind(id).await?.is_none() {
            return Err(StoreError::Conflict);
        }
        sqlx::query(
            "INSERT INTO session_attachment (state_id, attached_at) VALUES (?1, ?2)
             ON CONFLICT(state_id) DO UPDATE SET attached_at = excluded.attached_at",
        )
        .bind(id)
        .bind(attached_at)
        .execute(&mut self.connection)
        .await
        .map_err(crate::store::map_error)?;
        Ok(())
    }

    /// Releases a session so it may be purged again.
    pub async fn detach_session(&mut self, id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM session_attachment WHERE state_id = ?1")
            .bind(id)
            .execute(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        Ok(())
    }

    pub async fn is_attached(&mut self, id: &str) -> Result<bool, StoreError> {
        self.is_attached_raw(id).await
    }

    async fn is_attached_raw(&mut self, id: &str) -> Result<bool, StoreError> {
        let row = sqlx::query("SELECT 1 FROM session_attachment WHERE state_id = ?1")
            .bind(id)
            .fetch_optional(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        Ok(row.is_some())
    }

    async fn state_kind(&mut self, id: &str) -> Result<Option<String>, StoreError> {
        let row = sqlx::query("SELECT kind FROM state WHERE id = ?1")
            .bind(id)
            .fetch_optional(&mut self.connection)
            .await
            .map_err(crate::store::map_error)?;
        let Some(row) = row else { return Ok(None) };
        Ok(Some(
            row.try_get::<String, _>(0)
                .map_err(crate::store::map_error)?,
        ))
    }

    async fn event_totals(&mut self, id: &str) -> Result<(i64, i64), StoreError> {
        let row = sqlx::query(
            "SELECT COUNT(*), COALESCE(SUM(length(payload)), 0) FROM event WHERE state_id = ?1",
        )
        .bind(id)
        .fetch_one(&mut self.connection)
        .await
        .map_err(crate::store::map_error)?;
        Ok((
            row.try_get::<i64, _>(0).map_err(crate::store::map_error)?,
            row.try_get::<i64, _>(1).map_err(crate::store::map_error)?,
        ))
    }

    async fn artifact_rows_for(&mut self, session: &str) -> Result<Vec<ArtifactRow>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, kind, stored_bytes, content_hash FROM artifact WHERE state_id = ?1 ORDER BY id",
        )
        .bind(session)
        .fetch_all(&mut self.connection)
        .await
        .map_err(crate::store::map_error)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(ArtifactRow {
                id: row.try_get(0).map_err(crate::store::map_error)?,
                kind: row.try_get(1).map_err(crate::store::map_error)?,
                stored_bytes: row.try_get(2).map_err(crate::store::map_error)?,
                content_hash: row.try_get(3).map_err(crate::store::map_error)?,
            });
        }
        Ok(out)
    }

    /// Sessions outside the purge set that hold the same content hash.
    /// Sorted for determinism.
    async fn other_holders(
        &mut self,
        hash: &str,
        purge_set: &[&str],
    ) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query(
            "SELECT DISTINCT state_id FROM artifact WHERE content_hash = ?1 ORDER BY state_id",
        )
        .bind(hash)
        .fetch_all(&mut self.connection)
        .await
        .map_err(crate::store::map_error)?;
        let mut out = Vec::new();
        for row in rows {
            let state_id: String = row.try_get(0).map_err(crate::store::map_error)?;
            if !purge_set.contains(&state_id.as_str()) {
                out.push(state_id);
            }
        }
        Ok(out)
    }

    /// Commits the row removal for one session. Shared artifacts are
    /// re-owned by a surviving holder before the session row goes, so
    /// the foreign key never points at a removed session.
    async fn delete_session_rows(&mut self, session: &PurgeSessionPlan) -> Result<(), StoreError> {
        self.verify_drive()?;
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::store::map_error)?;
        for artifact in &session.preserved {
            let Some(owner) = artifact.held_by.first() else {
                continue;
            };
            sqlx::query("UPDATE artifact SET state_id = ?1 WHERE id = ?2")
                .bind(owner)
                .bind(&artifact.id)
                .execute(&mut *transaction)
                .await
                .map_err(crate::store::map_error)?;
        }
        let exclusive_ids: Vec<&str> = session.exclusive.iter().map(|a| a.id.as_str()).collect();
        for id in exclusive_ids {
            sqlx::query("DELETE FROM artifact WHERE id = ?1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(crate::store::map_error)?;
        }
        sqlx::query("DELETE FROM event WHERE state_id = ?1")
            .bind(&session.id)
            .execute(&mut *transaction)
            .await
            .map_err(crate::store::map_error)?;
        sqlx::query("DELETE FROM session_attachment WHERE state_id = ?1")
            .bind(&session.id)
            .execute(&mut *transaction)
            .await
            .map_err(crate::store::map_error)?;
        sqlx::query("DELETE FROM state WHERE id = ?1")
            .bind(&session.id)
            .execute(&mut *transaction)
            .await
            .map_err(crate::store::map_error)?;
        transaction
            .commit()
            .await
            .map_err(crate::store::map_error)?;
        Ok(())
    }

    /// Removes one artifact content file through the anchored managed
    /// root. Symlinks are refused rather than followed, so a planted link
    /// cannot point the delete at user data outside the drive.
    fn remove_artifact_file(&self, id: &str) -> Result<u64, StoreError> {
        if !crate::artifact::is_safe_artifact_id(id) {
            return Err(StoreError::UnsupportedPath);
        }
        #[cfg(test)]
        if PURGE_FILE_FAILURES.with(|failures| failures.borrow().contains(id)) {
            return Err(StoreError::Io);
        }
        let anchor = self.anchor_class(ARTIFACTS_DIRECTORY, false)?;
        let path = Store::anchored_path(&anchor, id);
        let metadata = fs::symlink_metadata(&path).map_err(|_| StoreError::Io)?;
        if metadata.is_symlink() {
            return Err(StoreError::Conflict);
        }
        if !metadata.is_file() {
            return Err(StoreError::Conflict);
        }
        let len = metadata.len();
        fs::remove_file(&path).map_err(|_| StoreError::Io)?;
        Ok(len)
    }
}
