//! Session YOLO and approval resolution that cannot hang.
//!
//! RUN-02c (PRD 12.2; Architecture A05). Composes the ledger with the
//! session's approval capability and produces the outcome the caller
//! acts on.
//!
//! Two properties drive the design.
//!
//! **YOLO removes prompts, not policy.** It is applied to the decision
//! after every denial and permission rule has run, and it only converts
//! `AskUser`. There is no code path here in which a `Deny` becomes an
//! `Allow`.
//!
//! **Headless is known before the request, not discovered by timeout.**
//! The host declares its capability when the service is built. A request
//! that would need a human in a host that has none returns immediately.
//! Nothing in this module reads a terminal, waits on a channel or holds
//! a deadline waiting for input, so "does not hang" is structural rather
//! than a timeout that could be set too long.

use fluzo_core::policy::{
    ExecutionBackend, PolicyDecision, PolicyOutcome, PolicyReason, PolicySubject,
    ResolvedPermission,
};

use crate::authorization::AuthorizationLedger;

/// Whether this host can ask a human anything.
///
/// Supplied by the host at construction. It is a capability of the
/// environment, not a preference: a TUI with a terminal is `Interactive`;
/// `--output json`, a pipe, a CI job and a detached process are
/// `Headless`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalMode {
    Interactive,
    Headless,
}

impl ApprovalMode {
    pub fn is_headless(self) -> bool {
        self == Self::Headless
    }
}

/// Something the interactive host is being asked to approve.
///
/// A value, not a callback. The caller renders it, collects the answer
/// and issues a grant through the ledger. Keeping it a value is what lets
/// the headless path produce the same shape without ever rendering it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    /// Redacted display string. Sensitive values are already masked by
    /// [`PolicySubject::redacted_summary`].
    pub summary: String,
    pub reason: PolicyReason,
}

/// What the caller may do now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthOutcome {
    /// Proceed. The reason says on whose authority: a grant, a
    /// permission, or YOLO.
    Allowed { reason: PolicyReason },
    /// Refused. Producing this never generates a prompt in any mode.
    Denied { reason: PolicyReason },
    /// An interactive host should prompt.
    ApprovalRequired(ApprovalRequest),
    /// Approval is needed and this host cannot obtain it.
    ///
    /// Distinct from `Denied` on purpose: the operation is not
    /// forbidden, it is unexecutable *here*. A headless caller reports
    /// this and moves on; conflating it with a denial would tell the
    /// operator they refused something they never saw.
    ApprovalUnavailable(ApprovalRequest),
}

impl AuthOutcome {
    pub fn allows(&self) -> bool {
        matches!(self, Self::Allowed { .. })
    }

    pub fn request(&self) -> Option<&ApprovalRequest> {
        match self {
            Self::ApprovalRequired(request) | Self::ApprovalUnavailable(request) => Some(request),
            _ => None,
        }
    }
}

/// Session authorization state: the ledger, the YOLO flag, the host
/// capability.
pub struct AuthorizationService {
    ledger: AuthorizationLedger,
    yolo: bool,
    mode: ApprovalMode,
}

impl AuthorizationService {
    pub fn new(mode: ApprovalMode) -> Self {
        Self {
            ledger: AuthorizationLedger::new(),
            yolo: false,
            mode,
        }
    }

    /// Turn session YOLO on or off.
    ///
    /// Process-memory only. There is no settings key for this, nothing
    /// writes it to disk, and a new service always starts with it off.
    pub fn set_yolo(&mut self, enabled: bool) {
        self.yolo = enabled;
    }

    pub fn yolo_enabled(&self) -> bool {
        self.yolo
    }

    pub fn mode(&self) -> ApprovalMode {
        self.mode
    }

    pub fn ledger(&self) -> &AuthorizationLedger {
        &self.ledger
    }

    pub fn ledger_mut(&mut self) -> &mut AuthorizationLedger {
        &mut self.ledger
    }

    /// Decide one proposed dispatch and resolve it against this host.
    ///
    /// Never blocks, in either mode. In headless mode an approval need
    /// becomes a returned value immediately; there is no read, wait or
    /// poll anywhere on this path.
    pub fn resolve(
        &self,
        approval_id: &str,
        subject: &PolicySubject,
        workspace: &str,
        backend: ExecutionBackend,
        permission: &ResolvedPermission,
    ) -> AuthOutcome {
        let decision = self.ledger.decide(subject, workspace, backend, permission);
        self.resolve_decision(approval_id, subject, decision)
    }

    /// Same, from a decision the caller already produced.
    ///
    /// Split out so the caller can audit the raw decision separately
    /// from what the host was able to do with it. The id comes from the
    /// caller because the caller owns operation identity; inventing one
    /// here would produce an id that correlates with nothing.
    pub fn resolve_decision(
        &self,
        approval_id: &str,
        subject: &PolicySubject,
        decision: PolicyDecision,
    ) -> AuthOutcome {
        let decision = if self.yolo {
            fluzo_core::policy::apply_yolo(decision, true)
        } else {
            decision
        };

        match decision.outcome {
            PolicyOutcome::Allow => AuthOutcome::Allowed {
                reason: decision.reason,
            },
            PolicyOutcome::Deny => AuthOutcome::Denied {
                reason: decision.reason,
            },
            PolicyOutcome::AskUser => {
                let request = ApprovalRequest {
                    id: approval_id.to_owned(),
                    summary: subject.redacted_summary(),
                    reason: decision.reason,
                };
                match self.mode {
                    ApprovalMode::Interactive => AuthOutcome::ApprovalRequired(request),
                    ApprovalMode::Headless => AuthOutcome::ApprovalUnavailable(request),
                }
            }
        }
    }

    /// Whether this subject could ever be approved in this session.
    ///
    /// A denied subject cannot, so an interactive host can skip prompting
    /// and a headless host can report without building a request.
    pub fn can_approve(&self, subject: &PolicySubject) -> bool {
        !self.ledger.subject_is_denied(subject)
    }
}

/// A structured headless refusal, shaped for a CLI exit.
///
/// PRD 12.2 and the bootstrap contract in `fluzo-cli`: a headless host
/// returns a code and a message rather than waiting for input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadlessApprovalRequired {
    pub code: &'static str,
    pub summary: String,
}

impl HeadlessApprovalRequired {
    pub const CODE: &'static str = "approval_required";

    /// Extract the headless outcome, if that is what this is.
    pub fn from_outcome(outcome: &AuthOutcome) -> Option<Self> {
        match outcome {
            AuthOutcome::ApprovalUnavailable(request) => Some(Self {
                code: Self::CODE,
                summary: request.summary.clone(),
            }),
            _ => None,
        }
    }
}
