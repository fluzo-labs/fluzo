//! The authorization ledger: live grants, current denials, and the
//! composed policy decision the runtime acts on.
//!
//! RUN-02b (PRD 12.1; Architecture A05). The pure rules live in
//! [`fluzo_core::policy`]; this is the state those rules are applied
//! to across a session.
//!
//! The ledger is deliberately in-memory. PRD 12 says task grants "MUST
//! NOT be restored automatically on recovery", so a restart starts with
//! no grants, which is the safe reading and the one that cannot be got
//! wrong by a migration. What *is* durable is the audit record of every
//! request, grant, consumption, denial, expiry and revocation, written
//! through the OBS-01 audit path by the caller. A denial that a restart
//! forgets has nothing left to protect against, because the grants it
//! denied are gone too.

use std::collections::{BTreeMap, BTreeSet};

use fluzo_core::policy::{
    Denial, ExecutionBackend, Grant, GrantLifetime, GrantState, PolicyDecision, PolicyOutcome,
    PolicySubject, ResolvedPermission, evaluate,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// A grant id that was never issued.
    UnknownGrant,
    /// The grant was already consumed by a specific dispatch. A second
    /// dispatch on one grant is refused rather than silently allowed:
    /// this is the case the one-time rule exists to stop.
    AlreadyConsumed { dispatch_id: String },
    /// The grant is not in a state that permits this transition.
    NotIssued { state: String },
    /// A denial id that was never added.
    UnknownDenial,
    /// An id already taken by another entry.
    DuplicateId,
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownGrant => formatter.write_str("unknown grant"),
            Self::AlreadyConsumed { dispatch_id } => {
                write!(
                    formatter,
                    "grant already consumed by dispatch {dispatch_id}"
                )
            }
            Self::NotIssued { state } => write!(formatter, "grant is {state}, not issued"),
            Self::UnknownDenial => formatter.write_str("unknown denial"),
            Self::DuplicateId => formatter.write_str("duplicate entry id"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Live authorization state for one session.
#[derive(Default)]
pub struct AuthorizationLedger {
    grants: BTreeMap<String, Grant>,
    denials: BTreeMap<String, Denial>,
    /// Grants whose dispatch outcome could not be confirmed.
    ///
    /// The grant stays consumed either way — that is the safe direction,
    /// because an un-consumed grant is a grant that can be replayed.
    /// This set exists so the unresolved state is visible rather than
    /// hidden behind a transition that already happened.
    uncertain: BTreeSet<String>,
}

impl AuthorizationLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn issue(&mut self, grant: Grant) -> Result<(), LedgerError> {
        if self.grants.contains_key(&grant.id) {
            return Err(LedgerError::DuplicateId);
        }
        self.grants.insert(grant.id.clone(), grant);
        Ok(())
    }

    pub fn add_denial(&mut self, denial: Denial) -> Result<(), LedgerError> {
        if self.denials.contains_key(&denial.id) {
            return Err(LedgerError::DuplicateId);
        }
        self.denials.insert(denial.id.clone(), denial);
        Ok(())
    }

    /// The first denial that matches `subject`.
    ///
    /// Any matching denial is enough: they all mean the same thing, and
    /// the first one found is reported so the operator can point at it.
    pub fn denial_for(&self, subject: &PolicySubject) -> Option<&Denial> {
        self.denials.values().find(|denial| denial.matches(subject))
    }

    /// The first live grant that covers `subject` exactly.
    pub fn grant_for(
        &self,
        subject: &PolicySubject,
        workspace: &str,
        backend: ExecutionBackend,
    ) -> Option<&Grant> {
        self.grants
            .values()
            .find(|grant| grant.covers(subject, workspace, backend))
    }

    /// Consume a grant for one dispatch, before that dispatch happens.
    ///
    /// Returns the consumed grant. Refuses if the grant is not `Issued`,
    /// which is what makes a second dispatch on a one-time grant
    /// impossible rather than merely discouraged.
    pub fn consume(&mut self, grant_id: &str, dispatch_id: &str) -> Result<Grant, LedgerError> {
        let grant = self
            .grants
            .get_mut(grant_id)
            .ok_or(LedgerError::UnknownGrant)?;
        match &grant.state {
            GrantState::Issued => {}
            GrantState::Consumed { dispatch_id } => {
                return Err(LedgerError::AlreadyConsumed {
                    dispatch_id: dispatch_id.clone(),
                });
            }
            other => {
                return Err(LedgerError::NotIssued {
                    state: describe_state(other),
                });
            }
        }
        grant.state = GrantState::Consumed {
            dispatch_id: dispatch_id.to_owned(),
        };
        Ok(grant.clone())
    }

    /// Records that a consumed grant's dispatch outcome is unresolved.
    ///
    /// The grant does not go back to `Issued`. It cannot. Callers report
    /// the uncertainty and leave the grant spent.
    pub fn mark_outcome_uncertain(&mut self, grant_id: &str) -> Result<(), LedgerError> {
        let grant = self.grants.get(grant_id).ok_or(LedgerError::UnknownGrant)?;
        if !grant.is_consumed() {
            return Err(LedgerError::NotIssued {
                state: describe_state(&grant.state),
            });
        }
        self.uncertain.insert(grant_id.to_owned());
        Ok(())
    }

    pub fn is_uncertain(&self, grant_id: &str) -> bool {
        self.uncertain.contains(grant_id)
    }

    /// Revoke a grant so it stops covering future dispatches.
    ///
    /// A completed dispatch is not undone. PRD 12: revocation affects
    /// future dispatches; it does not roll back work already done.
    pub fn revoke(&mut self, grant_id: &str, reason: &str) -> Result<Grant, LedgerError> {
        let grant = self
            .grants
            .get_mut(grant_id)
            .ok_or(LedgerError::UnknownGrant)?;
        if !matches!(grant.state, GrantState::Issued) {
            return Err(LedgerError::NotIssued {
                state: describe_state(&grant.state),
            });
        }
        grant.state = GrantState::Revoked {
            reason: reason.to_owned(),
        };
        Ok(grant.clone())
    }

    /// Expire every grant bound to a task. Called on completion,
    /// failure, cancellation and interruption.
    ///
    /// Returns how many grants expired, so the caller can report it.
    pub fn expire_task(&mut self, task_id: &str) -> usize {
        let mut expired = 0;
        for grant in self.grants.values_mut() {
            if matches!(&grant.lifetime, GrantLifetime::Task { task_id: id } if id == task_id)
                && matches!(grant.state, GrantState::Issued)
            {
                grant.state = GrantState::Expired;
                expired += 1;
            }
        }
        expired
    }

    /// The full decision for one proposed dispatch.
    ///
    /// Denial first, then grant, then the governing permission. Nothing
    /// here caches a decision: PRD 12 requires every dispatch, retries
    /// included, to be checked against current policy, and a cached
    /// answer is exactly how a revoked grant keeps working.
    pub fn decide(
        &self,
        subject: &PolicySubject,
        workspace: &str,
        backend: ExecutionBackend,
        permission: &ResolvedPermission,
    ) -> PolicyDecision {
        evaluate(
            subject,
            workspace,
            backend,
            permission,
            self.denial_for(subject),
            self.grant_for(subject, workspace, backend),
        )
    }

    pub fn grant(&self, grant_id: &str) -> Option<&Grant> {
        self.grants.get(grant_id)
    }

    pub fn live_grant_count(&self) -> usize {
        self.grants
            .values()
            .filter(|grant| matches!(grant.state, GrantState::Issued))
            .count()
    }

    /// Whether any live grant could ever cover this subject kind.
    ///
    /// Used by the approval surface to decide whether asking is pointless
    /// — a denied subject has no grant that could ever help, so the
    /// headless path can return immediately.
    pub fn subject_is_denied(&self, subject: &PolicySubject) -> bool {
        self.denial_for(subject).is_some()
    }
}

fn describe_state(state: &GrantState) -> String {
    match state {
        GrantState::Issued => "issued".to_owned(),
        GrantState::Consumed { .. } => "consumed".to_owned(),
        GrantState::Revoked { .. } => "revoked".to_owned(),
        GrantState::Expired => "expired".to_owned(),
    }
}

/// Whether a decision permits the dispatch to proceed.
pub fn permitted(decision: &PolicyDecision) -> bool {
    decision.outcome == PolicyOutcome::Allow
}
