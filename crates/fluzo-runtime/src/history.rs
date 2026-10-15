//! Synchronous facade over the store for the `fluzo history` CLI surface.
//!
//! `fluzo-cli` is not allowed a direct external dependency, so the async
//! runtime lives here rather than in the CLI. Each call builds a private
//! current-thread executor, runs one store operation and drops it.
//! Nothing here starts an agent, contacts a provider or runs a tool.

use crate::purge::{Confirmation, PurgeOutcome, PurgePlan, PurgeRefusal, SessionSummary};
use crate::store::{DRIVE_DIRECTORY, Store, StoreError};
use std::path::Path;

/// Why a history operation could not run.
#[derive(Debug)]
pub enum HistoryError {
    /// This repository has no drive yet, so there is no history.
    /// Reported rather than created as a side effect of listing.
    NoDrive,
    /// The store could not be opened or the operation failed.
    Store(StoreError),
    /// The private executor could not be built.
    Executor(String),
    /// The operation was refused. Nothing was deleted.
    Refused(PurgeRefusal),
}

impl HistoryError {
    /// Stable machine-readable code for the CLI exit path.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoDrive => "no_history",
            Self::Store(_) => "storage_unavailable",
            Self::Executor(_) => "runtime_unavailable",
            Self::Refused(PurgeRefusal::ConfirmationRequired) => "confirmation_required",
            Self::Refused(PurgeRefusal::SessionAttached(_)) => "session_attached",
            Self::Refused(PurgeRefusal::NoSuchSession(_)) => "no_such_session",
            Self::Refused(PurgeRefusal::NothingToPurge) => "nothing_to_purge",
        }
    }
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDrive => write!(f, "no .fluzodrive/ in this workspace"),
            Self::Store(error) => write!(f, "storage unavailable: {error:?}"),
            Self::Executor(message) => f.write_str(message),
            Self::Refused(refusal) => write!(f, "{refusal}"),
        }
    }
}

impl From<crate::purge::PurgeError> for HistoryError {
    fn from(error: crate::purge::PurgeError) -> Self {
        match error {
            crate::purge::PurgeError::Refused(refusal) => Self::Refused(refusal),
            crate::purge::PurgeError::Store(error) => Self::Store(error),
        }
    }
}

/// True when this workspace already has a database, so listing never
/// creates one as a side effect.
pub fn drive_present(root: &Path) -> bool {
    root.join(DRIVE_DIRECTORY)
        .join(crate::store::DATABASE_FILE)
        .exists()
}

fn executor() -> Result<tokio::runtime::Runtime, HistoryError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| HistoryError::Executor(format!("executor unavailable: {error}")))
}

/// Lists this repository's sessions. Read-only.
pub fn list(root: &Path) -> Result<Vec<SessionSummary>, HistoryError> {
    let runtime = executor()?;
    runtime.block_on(async move {
        let mut store = Store::open(root).await.map_err(HistoryError::Store)?;
        store.history_summary().await.map_err(HistoryError::Store)
    })
}

/// Builds a purge preview for `ids`. Never deletes.
pub fn plan(root: &Path, ids: &[String]) -> Result<PurgePlan, HistoryError> {
    let runtime = executor()?;
    let ids = ids.to_vec();
    runtime.block_on(async move {
        let mut store = Store::open(root).await.map_err(HistoryError::Store)?;
        store.plan_purge(&ids).await.map_err(HistoryError::from)
    })
}

/// Executes a purge. `confirmation` decides whether anything is removed.
pub fn purge(
    root: &Path,
    ids: &[String],
    confirmation: Confirmation,
) -> Result<PurgeOutcome, HistoryError> {
    let runtime = executor()?;
    let ids = ids.to_vec();
    runtime.block_on(async move {
        let mut store = Store::open(root).await.map_err(HistoryError::Store)?;
        store
            .purge_sessions(&ids, confirmation)
            .await
            .map_err(HistoryError::from)
    })
}

/// Finishes a purge that stopped after the rows were committed.
pub fn reclaim(root: &Path) -> Result<crate::purge::RemainderReport, HistoryError> {
    let runtime = executor()?;
    runtime.block_on(async move {
        let mut store = Store::open(root).await.map_err(HistoryError::Store)?;
        store
            .reclaim_orphan_artifact_files()
            .await
            .map_err(HistoryError::Store)
    })
}
