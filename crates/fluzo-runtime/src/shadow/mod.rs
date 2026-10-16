//! AGT-03 optional replaceable Laya shadow capability.
//!
//! The shadow observer sends bounded typed questions to a decision
//! model (`POST /v1/systemone`) and records what it answered. It is
//! strictly observational (PRD 10.1, Architecture A10):
//!
//! - A recommendation is inert data. No type path leads from a
//!   [`wire::ShadowAnswer`] to a grant, permission, routing decision
//!   or tool dispatch; the only output of this module is the
//!   [`ShadowObservationRecord`].
//! - The capability is disabled by default and cleanly detachable:
//!   nothing outside this module is required to remove it, and the
//!   coordinator depends only on the [`DecisionObserver`] trait, so a
//!   contract substitute needs no coordinator or UI change (A10).
//! - Every outcome is explicit: completed, rejected (protocol),
//!   failed, timed out, cancelled and skipped each carry their own
//!   status; late results stay linked to their original decision
//!   point.
//!
//! The Laya adapter composes the same gates as the AGT-01 chat
//! gateway — consent, admission (`ShadowObservation` skip-not-queue
//! semantics), credential references and the diagnostic hub — so
//! shadow traffic can never bypass the per-process limits (A04).
//!
//! Not included here: durable SQLite persistence of evaluation
//! records (DAT follow-up) and TUI wiring of the `decision.*`
//! settings (UI story). The coordinator exposes the status snapshot
//! those consumers need.

pub mod coordinator;
pub mod observer;
pub mod wire;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::watch;

use wire::{ShadowAnswer, ShadowQuestion};

/// The state and decision point observed before the corresponding
/// action. Observations must reference this snapshot, never a later
/// outcome (PRD 10.1).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DecisionPoint {
    pub task: String,
    pub stage: String,
    pub state_version: u64,
    pub state_fingerprint: String,
}

impl DecisionPoint {
    /// Deduplication identity: one observation per decision point.
    pub fn key(&self) -> (String, String, u64, String) {
        (
            self.task.clone(),
            self.stage.clone(),
            self.state_version,
            self.state_fingerprint.clone(),
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "task": self.task,
            "stage": self.stage,
            "state_version": self.state_version,
            "state_fingerprint": self.state_fingerprint,
        })
    }
}

/// One shadow observation request. Contains no authorization, no
/// callback and no shared mutable domain object.
pub struct ShadowRequest {
    pub observation_id: String,
    pub decision_point: DecisionPoint,
    pub question: ShadowQuestion,
    /// Decision-model alias recorded for evaluation provenance.
    pub model: String,
    /// Conservative input estimate owned by the caller (admission).
    pub input_estimate_tokens: u64,
    pub remaining_task_seconds: u64,
    pub state_size_bytes: u64,
    pub language: Option<String>,
    /// Description of any truncation or preprocessing applied to
    /// the observed state before asking (44.4).
    pub truncation: Option<String>,
}

impl ShadowRequest {
    /// Record skeleton carrying every provenance field, to be
    /// completed with a status. Used by every producer of records so
    /// the fields cannot drift between paths.
    pub fn base_record(&self) -> ShadowObservationRecord {
        ShadowObservationRecord {
            observation_id: self.observation_id.clone(),
            decision_point: self.decision_point.clone(),
            question_type: self.question.type_tag(),
            schema_version: wire::SYSTEMONE_SCHEMA_VERSION,
            candidate_actions: self.question.options().to_vec(),
            state_size_bytes: self.state_size_bytes,
            language: self.language.clone(),
            truncation: self.truncation.clone(),
            model: self.model.clone(),
            answer: None,
            recommended_action: None,
            actual_action: None,
            actual_action_reason: None,
            latency_ms: 0,
            status: ObservationStatus::Completed,
        }
    }
}

/// Explicit terminal status of one observation. Late, failed,
/// skipped and cancelled observations are recorded, never dropped
/// (PRD 10.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservationStatus {
    Completed,
    /// The response violated the wire schema (protocol failure).
    Rejected {
        reason: wire::AnswerRejection,
    },
    /// Transport, consent, credential or admission failure with a
    /// stable machine-readable code.
    Failed {
        code: String,
    },
    /// The whole-observation deadline elapsed.
    Timeout {
        stage: String,
    },
    /// Cancelled before or after dispatch.
    Cancelled {
        stage: String,
    },
    /// Skipped without a request (coordinator or admission rule).
    Skipped {
        reason: String,
    },
}

impl ObservationStatus {
    pub fn code(&self) -> String {
        match self {
            Self::Completed => "completed".to_owned(),
            Self::Rejected { reason } => reason.code().to_owned(),
            Self::Failed { code } => code.clone(),
            Self::Timeout { stage } => format!("timeout_{stage}"),
            Self::Cancelled { stage } => format!("cancelled_{stage}"),
            Self::Skipped { reason } => format!("skipped_{reason}"),
        }
    }

    /// Whether this status counts toward the automatic
    /// consecutive-failure pause. Only transport, protocol and
    /// timeout failures do (PRD 10.1); cancellations, skips,
    /// consent and admission denials never do.
    pub fn counts_as_failure(&self) -> bool {
        match self {
            Self::Completed | Self::Cancelled { .. } | Self::Skipped { .. } => false,
            Self::Rejected { .. } | Self::Timeout { .. } => true,
            Self::Failed { code } => code == "transport" || code == "response_too_large",
        }
    }
}

/// Durable-shaped evaluation record (PRD 44.4). Observation, actual
/// behavior and ground truth stay separate: `actual_action` and the
/// label fields are filled by later stages, never by the observer,
/// and this increment emits the record through the diagnostic
/// pipeline and the injectable sink rather than persisting it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShadowObservationRecord {
    pub observation_id: String,
    pub decision_point: DecisionPoint,
    pub question_type: &'static str,
    pub schema_version: u32,
    pub candidate_actions: Vec<String>,
    pub state_size_bytes: u64,
    pub language: Option<String>,
    pub truncation: Option<String>,
    pub model: String,
    pub answer: Option<ShadowAnswer>,
    pub recommended_action: Option<String>,
    /// Filled by the runtime when the actual action is known. The
    /// observer never writes it.
    pub actual_action: Option<String>,
    pub actual_action_reason: Option<String>,
    pub latency_ms: u64,
    pub status: ObservationStatus,
}

/// Narrow replacement boundary required by A10: the coordinator and
/// every consumer depend only on this trait. A substitute decision
/// adapter (different model, different protocol, a no-op) requires
/// no coordinator or UI change. Std-only boxed future so no extra
/// dependency enters the reviewed graph.
pub trait DecisionObserver: Send + Sync + 'static {
    /// Run one observation to its explicit terminal status. Must
    /// never block the caller's execution path: the coordinator
    /// invokes this from its own worker, and the `cancel` flag is a
    /// cooperative best-effort stop — a slot whose remote outcome is
    /// unconfirmed stays uncertain regardless (PRD 43.3).
    fn observe(
        self: Arc<Self>,
        request: ShadowRequest,
        cancel: watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = ShadowObservationRecord> + Send + 'static>>;
}

/// Consumer of completed records. Production wires the diagnostic
/// pipeline; tests collect.
pub trait ObservationSink: Send + Sync + 'static {
    fn submit(&self, record: ShadowObservationRecord);
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
