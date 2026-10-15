//! Synchronous facade over the store for the `fluzo history` CLI surface.
//!
//! `fluzo-cli` is not allowed a direct external dependency, so the async
//! runtime lives here rather than in the CLI.
//!
//! One `History` owns one runtime and one open store. A command must not
//! open the store once per step: the CLI needs a listing, a plan and a
//! delete, and reopening between them both shows the operator an
//! inconsistent snapshot and re-enters the WAL index lock while the
//! previous connection's deferred close may still be unwinding. Every
//! step here therefore runs against a single connection that is closed
//! deterministically by `History::close`.
//!
//! Nothing in this module starts an agent, contacts a provider or runs a
//! tool.

use crate::purge::{
    Confirmation, PurgeOutcome, PurgePlan, PurgeRefusal, RemainderReport, SessionSummary,
};
use crate::store::{DRIVE_DIRECTORY, Store, StoreError};
use std::path::Path;
use std::time::Duration;

/// Why a history operation could not run.
///
/// Store failures carry the stage that produced them. A bare `Busy` with
/// no stage cannot be diagnosed after the fact: it could be the connect,
/// the plan read, the delete transaction or the reclaim scan, and those
/// have different meanings. The stage is part of the message so a CI
/// failure names its own cause.
#[derive(Debug)]
pub enum HistoryError {
    /// This repository has no drive yet, so there is no history.
    /// Reported rather than created as a side effect of listing.
    NoDrive,
    /// A store operation failed at a named stage.
    Store {
        stage: &'static str,
        source: StoreError,
    },
    /// The private executor could not be built.
    Executor(String),
    /// The operation was refused. Nothing was deleted.
    Refused(PurgeRefusal),
}

impl HistoryError {
    fn store(stage: &'static str, source: StoreError) -> Self {
        Self::Store { stage, source }
    }

    /// Stable machine-readable code for the CLI exit path.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoDrive => "no_history",
            Self::Store { .. } => "storage_unavailable",
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
            Self::Store { stage, source } => {
                write!(f, "storage unavailable at {stage}: {source:?}")
            }
            Self::Executor(message) => f.write_str(message),
            Self::Refused(refusal) => write!(f, "{refusal}"),
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

/// A purge failure keeps its refusal identity, but a store failure inside
/// a purge is tagged with the stage that produced it.
fn describe(stage: &'static str, error: crate::purge::PurgeError) -> HistoryError {
    match error {
        crate::purge::PurgeError::Refused(refusal) => HistoryError::Refused(refusal),
        crate::purge::PurgeError::Store(source) => HistoryError::store(stage, source),
    }
}

/// One open store plus the runtime that drives it, for the lifetime of a
/// single `fluzo history` command.
pub struct History {
    runtime: tokio::runtime::Runtime,
    store: Store,
}

impl History {
    /// Opens the repository store once.
    ///
    /// Retries only the transient open-time BUSY that appears while a
    /// previous short-lived opener's WAL index lock is still being
    /// released -- the same class `connect_free_probe` already accepts
    /// for the restore probe. A store held by a live writer stays held:
    /// after the bounded attempts this still refuses with `Busy`, so the
    /// one-writer-per-workspace contract is unchanged. Nothing is waited
    /// on for any other error class, and no write path retries.
    pub fn open(root: &Path) -> Result<Self, HistoryError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| HistoryError::Executor(format!("executor unavailable: {error}")))?;
        let max_attempts = 4;
        let mut delay = Duration::from_millis(50);
        for attempt in 1..=max_attempts {
            match runtime.block_on(Store::open(root)) {
                Ok(store) => return Ok(Self { runtime, store }),
                Err(StoreError::Busy) if attempt < max_attempts => {
                    runtime.block_on(tokio::time::sleep(delay));
                    delay *= 2;
                }
                Err(error) => return Err(HistoryError::store("open", error)),
            }
        }
        Err(HistoryError::store("open", StoreError::Busy))
    }

    /// Lists this repository's sessions. Read-only.
    pub fn list(&mut self) -> Result<Vec<SessionSummary>, HistoryError> {
        self.runtime
            .block_on(self.store.history_summary())
            .map_err(|error| HistoryError::store("list", error))
    }

    /// Builds a purge preview for `ids`. Never deletes.
    pub fn plan(&mut self, ids: &[String]) -> Result<PurgePlan, HistoryError> {
        self.runtime
            .block_on(self.store.plan_purge(ids))
            .map_err(|error| describe("plan", error))
    }

    /// Executes a purge. `confirmation` decides whether anything is removed.
    pub fn purge(
        &mut self,
        ids: &[String],
        confirmation: Confirmation,
    ) -> Result<PurgeOutcome, HistoryError> {
        self.runtime
            .block_on(self.store.purge_sessions(ids, confirmation))
            .map_err(|error| describe("delete", error))
    }

    /// Finishes a purge that stopped after the rows were committed.
    pub fn reclaim(&mut self) -> Result<RemainderReport, HistoryError> {
        self.runtime
            .block_on(self.store.reclaim_orphan_artifact_files())
            .map_err(|error| HistoryError::store("reclaim", error))
    }

    /// Releases the store deterministically before the process exits.
    ///
    /// This is not optional cleanup. Dropping the connection defers the
    /// real close to a background thread, and a still-held WAL index
    /// lock makes the next opener fail with SQLITE_BUSY. The close result
    /// is returned rather than swallowed so a failed close is visible.
    pub fn close(self) -> Result<(), HistoryError> {
        let Self { runtime, store } = self;
        runtime
            .block_on(store.close())
            .map_err(|error| HistoryError::store("close", error))
    }
}
