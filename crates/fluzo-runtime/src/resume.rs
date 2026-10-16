//! RUN-04 P2: resume reconciliation.
//!
//! PRD 22.1 separates reopening (read-only, P1) from resuming (an
//! explicit user action). Resume revalidates the workspace and recorded
//! revisions, converts attempts that were still running into
//! `Interrupted`, keeps operations whose result was never confirmed
//! `Unknown`, carries known budget forward rather than resetting it,
//! drops the interrupted attempt's approvals, and records a new linked
//! attempt. It never rolls anything back and never replays a command.
//!
//! The split is deliberate: [`reconcile`] is computed through the
//! read-only P1 reader and performs no writes, so an inspection that
//! follows a resume request stays non-mutating until the reconciliation
//! is actually applied. [`apply`] is the only writer, it writes exactly
//! one atomic journal unit, and it refuses without confirmation.
//!
//! YOLO is absent from this module by construction. RUN-02 keeps it in
//! process memory with no settings key, so there is nothing persisted to
//! read and nothing here that could restore it;
//! [`resumed_authorization`] returns a service that starts with it off.

use crate::git_inspect::{self, Inspection};
use crate::recovery::{
    AttemptRecord, OperationRecord, RecoveryReader, SessionRecord, journal_record_payload,
};
use crate::session_auth::{ApprovalMode, AuthorizationService};
use crate::store::{EventRecord, StateUpdate, Store, StoreError};
use crate::workspace::{OwnershipError, WorkspaceIdentity};
use fluzo_core::application::{AttemptState, OperationState};
use std::path::Path;

/// Upper bound on how far a resume may be renumbered before the caller
/// must intervene, so a corrupted attempt set cannot produce an
/// unbounded id sequence.
pub const MAX_ATTEMPTS_PER_SESSION: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeError {
    /// A resume changes files, so it requires an explicit confirmation.
    /// Returned rather than waited on: a non-interactive caller must not
    /// block, and there is no bypass flag that skips this.
    ConfirmationRequired,
    /// The reconciliation was computed against a different session than
    /// the one being applied.
    SessionMismatch { expected: String, found: String },
    /// This reconciliation has already been applied. Reported instead of
    /// writing a second attempt, so an interrupted retry converges.
    AlreadyApplied { attempt_id: String },
    /// The attempt set exceeded the renumbering bound.
    TooManyAttempts { session_id: String, bound: usize },
    Store(StoreError),
}

impl std::fmt::Display for ResumeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfirmationRequired => {
                write!(formatter, "resume requires explicit confirmation")
            }
            Self::SessionMismatch { expected, found } => {
                write!(formatter, "reconciliation is for {expected:?}, not {found:?}")
            }
            Self::AlreadyApplied { attempt_id } => {
                write!(formatter, "resume already applied as {attempt_id:?}")
            }
            Self::TooManyAttempts { session_id, bound } => {
                write!(formatter, "{session_id:?} exceeds {bound} attempts")
            }
            Self::Store(error) => write!(formatter, "store: {error}"),
        }
    }
}

impl std::error::Error for ResumeError {}

impl From<StoreError> for ResumeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Why a session cannot be resumed at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileBlock {
    /// The live workspace is a different directory than the one the
    /// session was recorded against.
    ///
    /// This blocks rather than proceeding: PRD 22.1 forbids silently
    /// substituting the runtime's current startup directory, because
    /// the resumed work would then edit a tree the user never approved.
    WorkspaceMismatch { recorded: String, observed: String },
    /// The workspace could not be resolved, so identity is unknown and
    /// unknown identity is not permission.
    WorkspaceUnresolved(String),
    /// The live Git revision could not be determined.
    RevisionUnknown(String),
    /// The session could not be read, so there is nothing to reconcile.
    SessionUnavailable(String),
}

/// A recorded claim that the current workspace no longer supports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaleClaim {
    /// A verification recorded at one revision is being resumed at
    /// another, so the verification result no longer describes the tree.
    VerificationRevision {
        recorded: String,
        observed: String,
        subject: String,
    },
    /// An edit approval was recorded against a revision the tree has
    /// since moved past. The approval is invalidated, not carried over.
    EditApproval {
        recorded: String,
        observed: String,
        subject: String,
    },
}

/// Budget carried into the resumed attempt.
///
/// Known consumption is carried forward exactly. A contribution that
/// was uncertain is added to the totals as its recorded figure and the
/// whole carry-forward is flagged uncertain, so the caller cannot read
/// the number as a trustworthy remaining-budget basis. Extending the
/// budget is a separate explicit act, never implicit in resuming.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetCarryForward {
    pub tokens: u64,
    pub active_seconds: u64,
    pub tool_calls: u64,
    pub turns: u64,
    pub uncertain: bool,
}

/// The read-only result of reconciling one session against the live
/// workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciliation {
    pub session_id: String,
    /// Attempts recorded as running that resume converts to interrupted.
    pub interrupted_attempts: Vec<String>,
    /// Operations that may have acted with no confirmed result.
    pub unknown_operations: Vec<String>,
    /// Grants held by the interrupted attempt. They do not carry over.
    pub expired_grants: Vec<String>,
    /// Approvals that were pending when the attempt stopped.
    pub dropped_pending_approvals: Vec<String>,
    pub stale_claims: Vec<StaleClaim>,
    pub carry_forward: BudgetCarryForward,
    pub prior_attempt_id: Option<String>,
    pub new_attempt_id: String,
}

/// The durable result of applying a reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeApplied {
    pub reconciliation: Reconciliation,
    pub attempt_id: String,
    pub linked_prior_attempt_id: Option<String>,
}

/// Observed live-workspace facts to reconcile against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveWorkspace {
    pub fingerprint: String,
    pub git_revision: Option<String>,
}

impl LiveWorkspace {
    /// Reads the live workspace fingerprint and resolves `HEAD`.
    ///
    /// The revision comes from the RUN-03 closed-set `RevParse`
    /// inspection rather than a bespoke git call, so resume introduces
    /// no new executable surface.
    pub fn observe(root: &Path) -> Result<Self, ReconcileBlock> {
        let fingerprint = WorkspaceIdentity::resolve(root)
            .map(|identity| identity.fingerprint())
            .map_err(|error| ReconcileBlock::WorkspaceUnresolved(describe_identity_error(&error)))?;
        let git_revision = match git_inspect::inspect(
            root,
            &Inspection::RevParse {
                rev: "HEAD".to_owned(),
            },
        ) {
            Ok(revision) => Some(revision.trim().to_owned()).filter(|value| !value.is_empty()),
            Err(error) => {
                return Err(ReconcileBlock::RevisionUnknown(format!("{error:?}")));
            }
        };
        Ok(Self {
            fingerprint,
            git_revision,
        })
    }
}

fn describe_identity_error(error: &OwnershipError) -> String {
    format!("{error:?}")
}

/// Computes the reconciliation for one session without writing anything.
///
/// Reads only through [`RecoveryReader`], which holds a read-only
/// connection, so "inspection stays non-mutating until the
/// reconciliation is complete" (PRD 22.1) holds at the driver level.
pub async fn reconcile(
    reader: &mut RecoveryReader,
    session_id: &str,
    live: &LiveWorkspace,
) -> Result<Reconciliation, ReconcileBlock> {
    let session = reader
        .session(session_id)
        .await
        .map_err(|error| ReconcileBlock::SessionUnavailable(format!("{error}")))?;

    // Identity first: nothing else is worth computing if the tree is not
    // the one the user approved.
    if session.session.workspace_fingerprint != live.fingerprint {
        return Err(ReconcileBlock::WorkspaceMismatch {
            recorded: session.session.workspace_fingerprint.clone(),
            observed: live.fingerprint.clone(),
        });
    }

    let observed_revision = live
        .git_revision
        .clone()
        .ok_or_else(|| ReconcileBlock::RevisionUnknown("no revision supplied".to_owned()))?;

    let mut reconciliation = Reconciliation {
        session_id: session_id.to_owned(),
        interrupted_attempts: Vec::new(),
        unknown_operations: Vec::new(),
        expired_grants: Vec::new(),
        dropped_pending_approvals: Vec::new(),
        stale_claims: Vec::new(),
        carry_forward: BudgetCarryForward::default(),
        prior_attempt_id: None,
        new_attempt_id: String::new(),
    };

    if session.attempts.len() >= MAX_ATTEMPTS_PER_SESSION {
        return Err(ReconcileBlock::WorkspaceUnresolved(format!(
            "attempt set exceeded {MAX_ATTEMPTS_PER_SESSION}"
        )));
    }

    // The prior attempt is the latest one by id, which is the ordering
    // the reader guarantees.
    if let Some(last) = session.attempts.last() {
        reconciliation.prior_attempt_id = Some(last.attempt_id.clone());
    }
    reconciliation.new_attempt_id = next_attempt_id(session_id, session.attempts.len());

    for attempt in &session.attempts {
        if attempt.effective_state() == AttemptState::Interrupted {
            reconciliation
                .interrupted_attempts
                .push(attempt.attempt_id.clone());
        }
        // Budget carries forward from every prior attempt, including an
        // interrupted one: the work was paid for.
        let budget = &attempt.budget;
        reconciliation.carry_forward.tokens = reconciliation.carry_forward.tokens.saturating_add(budget.tokens);
        reconciliation.carry_forward.active_seconds =
            reconciliation.carry_forward.active_seconds.saturating_add(budget.active_seconds);
        reconciliation.carry_forward.tool_calls =
            reconciliation.carry_forward.tool_calls.saturating_add(budget.tool_calls);
        reconciliation.carry_forward.turns =
            reconciliation.carry_forward.turns.saturating_add(budget.turns);
        reconciliation.carry_forward.uncertain |= budget.uncertain;
    }

    for operation in &session.operations {
        match operation.effective_state() {
            OperationState::Unknown => reconciliation
                .unknown_operations
                .push(operation.operation_id.clone()),
            OperationState::AwaitingApproval => reconciliation
                .dropped_pending_approvals
                .push(operation.operation_id.clone()),
            _ => {}
        }
        if let Some(grant_id) = &operation.grant_id {
            // A grant consumed by the interrupted attempt is not
            // reissued. Standing grants are a configuration concern and
            // are re-derived from current policy, never carried here.
            if !matches!(
                operation.effective_state(),
                OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
            ) {
                reconciliation.expired_grants.push(grant_id.clone());
            }
        }
        // A recorded verification or approval against a revision the
        // tree has moved past is invalidated and surfaced.
        if let Some(recorded) = &session.session.git_revision {
            if *recorded != observed_revision {
                let claim = match operation.effective_state() {
                    OperationState::Succeeded => Some(StaleClaim::VerificationRevision {
                        recorded: recorded.clone(),
                        observed: observed_revision.clone(),
                        subject: operation.subject.clone(),
                    }),
                    _ => Some(StaleClaim::EditApproval {
                        recorded: recorded.clone(),
                        observed: observed_revision.clone(),
                        subject: operation.subject.clone(),
                    }),
                };
                if let Some(claim) = claim {
                    reconciliation.stale_claims.push(claim);
                }
            }
        }
    }

    Ok(reconciliation)
}

/// Deterministic successor attempt id.
pub fn next_attempt_id(session_id: &str, existing: usize) -> String {
    format!("{session_id}-attempt-{:04}", existing + 1)
}

/// Applies a reconciliation in one atomic journal unit.
///
/// Refuses without explicit confirmation and never waits, so a headless
/// caller gets a structured outcome instead of blocking on a prompt it
/// cannot answer. Re-applying the same reconciliation is a no-op that
/// reports the existing attempt rather than creating a duplicate.
pub async fn apply(
    store: &mut Store,
    reconciliation: &Reconciliation,
    confirmed: bool,
) -> Result<ResumeApplied, ResumeError> {
    if !confirmed {
        return Err(ResumeError::ConfirmationRequired);
    }
    let existing = store
        .state(&reconciliation.new_attempt_id)
        .await
        .map_err(ResumeError::Store)?;
    if existing.is_some() {
        return Err(ResumeError::AlreadyApplied {
            attempt_id: reconciliation.new_attempt_id.clone(),
        });
    }

    let session: SessionRecord = match store
        .state(&reconciliation.session_id)
        .await
        .map_err(ResumeError::Store)?
    {
        Some((_, payload, _)) => {
            decode_session_payload(&reconciliation.session_id, &payload).ok_or_else(|| {
                ResumeError::SessionMismatch {
                    expected: reconciliation.session_id.clone(),
                    found: "unreadable session record".to_owned(),
                }
            })?
        }
        None => {
            return Err(ResumeError::SessionMismatch {
                expected: reconciliation.session_id.clone(),
                found: "no session record".to_owned(),
            });
        }
    };

    let mut updates: Vec<StateUpdate> = Vec::new();
    let mut events: Vec<EventRecord> = Vec::new();

    // Re-read the interrupted attempts and rewrite their durable state to
    // interrupted, so a later reader never sees a stale "running".
    for attempt_id in &reconciliation.interrupted_attempts {
        if let Some((_, payload, _)) = store.state(attempt_id).await.map_err(ResumeError::Store)? {
            let mut record: AttemptRecord = decode_attempt_payload(attempt_id, &payload).ok_or_else(
                || ResumeError::SessionMismatch {
                    expected: attempt_id.clone(),
                    found: "unreadable attempt record".to_owned(),
                },
            )?;
            record.state = AttemptState::Interrupted;
            record.ended_at_ms = Some(now_ms());
            updates.push(StateUpdate {
                id: attempt_id.clone(),
                kind: crate::recovery::ATTEMPT_KIND.to_owned(),
                payload: record.encode(),
            });
        }
    }

    for operation_id in &reconciliation.unknown_operations {
        if let Some((_, payload, _)) = store.state(operation_id).await.map_err(ResumeError::Store)? {
            let mut record: OperationRecord =
                decode_operation_payload(operation_id, &payload).ok_or_else(|| {
                    ResumeError::SessionMismatch {
                        expected: operation_id.clone(),
                        found: "unreadable operation record".to_owned(),
                    }
                })?;
            record.state = OperationState::Unknown;
            updates.push(StateUpdate {
                id: operation_id.clone(),
                kind: crate::recovery::OPERATION_KIND.to_owned(),
                payload: record.encode(),
            });
        }
    }

    // The new attempt carries the carried-forward budget and no grants:
    // nothing from the stopped attempt is reissued with it.
    let new_attempt = AttemptRecord {
        attempt_id: reconciliation.new_attempt_id.clone(),
        session_id: reconciliation.session_id.clone(),
        task_id: format!("task-of-{}", reconciliation.prior_attempt_id.as_deref().unwrap_or("root")),
        prior_attempt_id: reconciliation.prior_attempt_id.clone(),
        state: AttemptState::Pending,
        started_at_ms: now_ms(),
        ended_at_ms: None,
        budget: crate::recovery::BudgetRecord {
            tokens: reconciliation.carry_forward.tokens,
            active_seconds: reconciliation.carry_forward.active_seconds,
            tool_calls: reconciliation.carry_forward.tool_calls,
            turns: reconciliation.carry_forward.turns,
            uncertain: reconciliation.carry_forward.uncertain,
        },
    };
    updates.push(StateUpdate {
        id: new_attempt.attempt_id.clone(),
        kind: crate::recovery::ATTEMPT_KIND.to_owned(),
        payload: new_attempt.encode(),
    });

    // The reconciliation itself is durable and observable: what was
    // interrupted, what stayed unknown, what was dropped as stale.
    let summary = format!(
        "resume: workspace={} interrupted={} unknown={} grants_expired={} dropped_approvals={} stale={} budget_uncertain={}",
        session.workspace_fingerprint,
        reconciliation.interrupted_attempts.len(),
        reconciliation.unknown_operations.len(),
        reconciliation.expired_grants.len(),
        reconciliation.dropped_pending_approvals.len(),
        reconciliation.stale_claims.len(),
        reconciliation.carry_forward.uncertain,
    );
    let ordinal = reconciliation.interrupted_attempts.len() as u64
        + reconciliation.unknown_operations.len() as u64
        + 1;
    events.push(EventRecord {
        state_id: reconciliation.session_id.clone(),
        kind: "resume".to_owned(),
        payload: journal_record_payload(
            ordinal,
            now_ms(),
            &summary,
            Some(&new_attempt.attempt_id),
            None,
        ),
    });
    for claim in &reconciliation.stale_claims {
        events.push(EventRecord {
            state_id: reconciliation.session_id.clone(),
            kind: "stale_claim".to_owned(),
            payload: journal_record_payload(
                ordinal + events.len() as u64 + 1,
                now_ms(),
                &describe_claim(claim),
                Some(&new_attempt.attempt_id),
                None,
            ),
        });
    }

    // One unit: the state transitions and the resume record commit
    // together or not at all.
    store.commit_unit(&updates, &events).await?;

    Ok(ResumeApplied {
        reconciliation: reconciliation.clone(),
        attempt_id: new_attempt.attempt_id,
        linked_prior_attempt_id: reconciliation.prior_attempt_id.clone(),
    })
}

/// A resumed session's authorization service.
///
/// YOLO is off because it is never persisted: RUN-02 keeps it in
/// process memory with no settings key, so a resumed process has no
/// value to restore even if one asked. This function exists to make
/// that the only available path rather than a convention callers are
/// asked to honour.
pub fn resumed_authorization(mode: ApprovalMode) -> AuthorizationService {
    AuthorizationService::new(mode)
}

fn describe_claim(claim: &StaleClaim) -> String {
    match claim {
        StaleClaim::VerificationRevision {
            recorded,
            observed,
            subject,
        } => format!("verification of {subject} recorded at {recorded} is invalid at {observed}"),
        StaleClaim::EditApproval {
            recorded,
            observed,
            subject,
        } => format!("edit approval for {subject} recorded at {recorded} is invalid at {observed}"),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

/// Decoders mirroring the P1 encoders. Kept here rather than exported
/// from `recovery` because the apply path needs owned mutable records.
fn decode_session_payload(id: &str, payload: &str) -> Option<SessionRecord> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    Some(SessionRecord {
        session_id: string_field(object, "session_id").unwrap_or_else(|| id.to_owned()),
        workspace_fingerprint: string_field(object, "workspace_fingerprint")?,
        git_branch: string_field(object, "git_branch"),
        git_revision: string_field(object, "git_revision"),
        created_at_ms: number_field(object, "created_at_ms"),
        updated_at_ms: number_field(object, "updated_at_ms"),
    })
}

fn decode_attempt_payload(id: &str, payload: &str) -> Option<AttemptRecord> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    Some(AttemptRecord {
        attempt_id: string_field(object, "attempt_id").unwrap_or_else(|| id.to_owned()),
        session_id: string_field(object, "session_id")?,
        task_id: string_field(object, "task_id")?,
        prior_attempt_id: string_field(object, "prior_attempt_id"),
        state: serde_json::from_value(object.get("state")?.clone()).ok()?,
        started_at_ms: number_field(object, "started_at_ms"),
        ended_at_ms: object.get("ended_at_ms").and_then(|v| v.as_u64()),
        budget: {
            let budget = object.get("budget")?.as_object()?;
            crate::recovery::BudgetRecord {
                tokens: number_field(budget, "tokens"),
                active_seconds: number_field(budget, "active_seconds"),
                tool_calls: number_field(budget, "tool_calls"),
                turns: number_field(budget, "turns"),
                uncertain: budget
                    .get("uncertain")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            }
        },
    })
}

fn decode_operation_payload(id: &str, payload: &str) -> Option<OperationRecord> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let object = value.as_object()?;
    Some(OperationRecord {
        operation_id: string_field(object, "operation_id").unwrap_or_else(|| id.to_owned()),
        attempt_id: string_field(object, "attempt_id")?,
        session_id: string_field(object, "session_id")?,
        subject: string_field(object, "subject")?,
        state: serde_json::from_value(object.get("state")?.clone()).ok()?,
        effect_committed: object
            .get("effect_committed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        result_known: object.get("result_known").and_then(|v| v.as_bool()).unwrap_or(false),
        grant_id: string_field(object, "grant_id"),
    })
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    object
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|value| !value.is_empty() && value != "none")
}

fn number_field(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> u64 {
    object
        .get(key)
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}
