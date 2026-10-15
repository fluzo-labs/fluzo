//! Policy evaluation: origin-aware permission resolution, denial
//! precedence and the exact-scope grant model.
//!
//! This is the RUN-02 decision layer (PRD 12, 26.1; Architecture A05).
//! It is pure: no I/O, no clock, no terminal. The runtime calls it and
//! acts on the answer; nothing here can execute, prompt or persist.
//!
//! Three rules shape the whole module.
//!
//! **Denials win.** An explicit user denial outranks a matching grant
//! and outranks an `allow` from any origin. That is checked first, so no
//! ordering of grants can work around it.
//!
//! **A grant covers exactly what it was given for.** Matching is exact
//! equality on the scope. PRD 12 says approval MUST NOT extend to
//! command prefixes, extra arguments, chained commands or other
//! directories. Implementing that as "the scope values must be equal"
//! is the only version that holds; a matcher clever enough to generalise
//! is the bug.
//!
//! **Repository content is a request, not authority.** A `.fluzo` may
//! narrow a permission. It may never widen one. See
//! [`resolve_permission`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::settings::Permission;

/// Where a permission value came from.
///
/// This mirrors the origins in [`crate::settings::SettingOrigin`] that
/// can carry a policy value, narrowed to the ones the policy table
/// actually distinguishes. The distinction that matters is a single
/// bit: whether the value is the operator speaking, or repository
/// content asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PermissionOrigin {
    /// The registry default.
    BuiltIn,
    /// `.fluzo` in the repository. A request, never authority to widen.
    Repository,
    /// An operator-owned configuration file.
    ExplicitFile,
    /// A command-line override for this invocation.
    CommandLine,
}

impl PermissionOrigin {
    /// Whether a value from this origin may make a permission wider than
    /// it currently is.
    ///
    /// Only the repository is barred. Everything else is the operator
    /// speaking directly, and PRD 26.1 bars the repository from
    /// broadening grants without user authorization.
    pub fn may_broaden(self) -> bool {
        self != Self::Repository
    }

    /// Priority among operator origins. Higher wins when two operator
    /// layers disagree.
    const fn operator_rank(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            // Never ranked as an operator layer; applied only as a
            // narrowing below.
            Self::Repository => 1,
            Self::ExplicitFile => 2,
            Self::CommandLine => 3,
        }
    }
}

/// One candidate value for a permission, with where it came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PermissionLayer {
    pub value: Permission,
    pub origin: PermissionOrigin,
}

impl PermissionLayer {
    pub fn new(value: Permission, origin: PermissionOrigin) -> Self {
        Self { value, origin }
    }
}

/// How restrictive a permission is. Higher is stricter.
///
/// The ordering makes "the repository may only narrow" a comparison
/// rather than a per-key special case.
const fn restrictiveness(permission: Permission) -> u8 {
    match permission {
        Permission::Allow => 0,
        Permission::Ask => 1,
        Permission::Deny => 2,
    }
}

/// A permission after every layer has been combined.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPermission {
    /// The value that governs.
    pub value: Permission,
    /// Which layer supplied the governing value.
    pub origin: PermissionOrigin,
    /// A repository value that asked for more than the operator allowed.
    ///
    /// Recorded rather than silently dropped. A `.fluzo` that tried to
    /// widen a permission should be visible to the operator; dropping it
    /// quietly would make the file's effect unknowable.
    pub ignored_repository_broaden: Option<Permission>,
}

/// Combine permission layers.
///
/// The operator's value is the highest-priority non-repository layer.
/// Each repository layer is then applied only if it is *more*
/// restrictive. A repository layer that is less restrictive is ignored
/// and recorded in `ignored_repository_broaden`.
///
/// A repository narrowing applies even over a command-line `allow`.
/// PRD 26.2 says CLI overrides remain authoritative for the invocation,
/// but that is about configuration value precedence, not about defeating
/// a repository's right to narrow; PRD 12's "denials win" is the
/// stronger rule, and the narrowed value is reported with
/// `origin = Repository` so the operator can see who narrowed it.
pub fn resolve_permission(layers: &[PermissionLayer]) -> ResolvedPermission {
    let operator = layers
        .iter()
        .filter(|layer| layer.origin != PermissionOrigin::Repository)
        .max_by_key(|layer| layer.origin.operator_rank());

    let mut value = operator.map_or(Permission::Ask, |layer| layer.value);
    let mut origin = operator.map_or(PermissionOrigin::BuiltIn, |layer| layer.origin);
    let mut ignored_repository_broaden = None;

    for layer in layers
        .iter()
        .filter(|layer| layer.origin == PermissionOrigin::Repository)
    {
        if restrictiveness(layer.value) > restrictiveness(value) {
            value = layer.value;
            origin = PermissionOrigin::Repository;
        } else if restrictiveness(layer.value) < restrictiveness(value) {
            ignored_repository_broaden = Some(layer.value);
        }
    }

    ResolvedPermission {
        value,
        origin,
        ignored_repository_broaden,
    }
}

/// What kind of thing is being proposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum SubjectKind {
    Filesystem,
    Shell,
    Network,
    Git,
    Credential,
    Unknown,
}

/// A filesystem operation on one path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileOperation {
    Read,
    Write,
    Create,
    Delete,
    Rename,
}

/// An observed file identity, used to detect a change between approval
/// and dispatch.
///
/// PRD 12: "approvals for pre-existing changes MUST also match the
/// observed file version". The runtime fills this from the RUN-01 jail;
/// core only defines the shape, so the comparison is exact and a grant
/// stops covering the moment the file moves under it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileVersion {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_ns: i64,
}

/// A workspace-relative path plus the operation proposed on it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileScope {
    pub operation: FileOperation,
    pub path: String,
    /// The version the operator saw when they approved. A grant bound to
    /// a version stops covering any other version.
    pub observed: Option<FileVersion>,
}

impl FileScope {
    pub fn new(operation: FileOperation, path: impl Into<String>) -> Self {
        Self {
            operation,
            path: path.into(),
            observed: None,
        }
    }

    pub fn with_observed(mut self, observed: FileVersion) -> Self {
        self.observed = Some(observed);
        self
    }
}

/// How a command is being run.
///
/// A05 makes direct execution the default and the shell an explicit
/// selection. They are different scopes: approving one says nothing
/// about the other, because a shell interprets its argument and a direct
/// request does not.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ShellMode {
    Direct,
    ExplicitShell,
}

/// A command proposal.
///
/// PRD 12: "Shell scope MUST include the exact command or
/// executable/arguments, shell mode, working directory, and explicitly
/// supplied environment overrides."
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ShellScope {
    pub mode: ShellMode,
    pub executable: String,
    pub arguments: Vec<String>,
    /// Workspace-relative.
    pub working_directory: String,
    /// Explicit overrides only. Values are sensitive and never printed.
    pub environment: BTreeMap<String, String>,
}

impl ShellScope {
    pub fn direct(
        executable: impl Into<String>,
        arguments: Vec<String>,
        working_directory: impl Into<String>,
    ) -> Self {
        Self {
            mode: ShellMode::Direct,
            executable: executable.into(),
            arguments,
            working_directory: working_directory.into(),
            environment: BTreeMap::new(),
        }
    }
}

/// A Git operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GitOperation {
    Commit,
    Push,
}

/// A tool network destination.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkScope {
    pub destination: String,
}

/// The operation being proposed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PolicySubject {
    Filesystem(FileScope),
    Shell(ShellScope),
    Network(NetworkScope),
    Git(GitOperation),
    Credential {
        reference: String,
    },
    /// A subject this build of the policy engine does not know about.
    /// Fails closed to `AskUser` rather than defaulting to allowed.
    Unknown {
        kind: String,
    },
}

impl PolicySubject {
    pub fn kind(&self) -> SubjectKind {
        match self {
            Self::Filesystem(_) => SubjectKind::Filesystem,
            Self::Shell(_) => SubjectKind::Shell,
            Self::Network(_) => SubjectKind::Network,
            Self::Git(_) => SubjectKind::Git,
            Self::Credential { .. } => SubjectKind::Credential,
            Self::Unknown { .. } => SubjectKind::Unknown,
        }
    }

    /// The registry key that governs this subject, if any.
    ///
    /// `None` means the permission table has nothing to say about it, so
    /// the engine fails closed to `AskUser` rather than inventing a
    /// default. Filesystem access in particular is decided by the RUN-01
    /// jail, not by a permission knob, and adding a `policy.filesystem`
    /// key here would create a second and weaker authority over
    /// something the jail already refuses.
    pub fn policy_key(&self) -> Option<&'static str> {
        match self {
            Self::Shell(_) => Some("policy.shell"),
            Self::Network(_) => Some("policy.tool_network"),
            Self::Git(GitOperation::Commit) => Some("policy.git_commit"),
            Self::Git(GitOperation::Push) => Some("policy.git_push"),
            Self::Filesystem(_) | Self::Credential { .. } | Self::Unknown { .. } => None,
        }
    }

    /// A display/audit string with sensitive values masked.
    ///
    /// PRD 12: "Sensitive values MUST be redacted in approval displays
    /// and audit events." Environment values never appear; only their
    /// names do, because the operator needs to see that an override was
    /// proposed without the display leaking its content.
    pub fn redacted_summary(&self) -> String {
        match self {
            Self::Filesystem(scope) => {
                let bound = if scope.observed.is_some() {
                    " [version-bound]"
                } else {
                    ""
                };
                format!("filesystem {:?} {:?}{bound}", scope.operation, scope.path)
            }
            Self::Shell(scope) => {
                let mode = match scope.mode {
                    ShellMode::Direct => "direct",
                    ShellMode::ExplicitShell => "shell",
                };
                let env = if scope.environment.is_empty() {
                    String::new()
                } else {
                    let names: Vec<&str> = scope.environment.keys().map(String::as_str).collect();
                    format!(" env[{}]=[redacted]", names.join(","))
                };
                format!(
                    "shell({mode}) {:?} {:?} cwd={:?}{env}",
                    scope.executable, scope.arguments, scope.working_directory
                )
            }
            Self::Network(scope) => format!("network {:?}", scope.destination),
            Self::Git(operation) => format!("git {:?}", operation),
            Self::Credential { reference } => format!("credential {reference}"),
            Self::Unknown { kind } => format!("unknown({kind})"),
        }
    }
}

/// What a grant was given for, as far as lifetime is concerned.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GrantLifetime {
    /// Exactly one dispatch, then done.
    OneTime,
    /// Any dispatch of the same operation for the named task, until the
    /// task ends. PRD 12 lets a user authorize matching operations for
    /// the current task instead of once.
    Task { task_id: String },
}

/// Where a grant is in its life.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GrantState {
    Issued,
    /// Consumed by a specific dispatch. There is no transition back: a
    /// grant that could be un-consumed is a grant that can be replayed.
    Consumed {
        dispatch_id: String,
    },
    Revoked {
        reason: String,
    },
    /// Its task completed, failed, was cancelled or was interrupted.
    Expired,
}

/// The backend a grant authorizes work on.
///
/// A grant is bound to a backend so that authorizing native execution
/// cannot later be read as authorizing an explicitly shelled command,
/// or work on some other execution target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ExecutionBackend {
    NativeLocal,
    ExplicitShell,
}

/// An authorization for one exact operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    /// The exact operation this was given for.
    pub subject: PolicySubject,
    /// The workspace fingerprint this is bound to. A fingerprint rather
    /// than a path, so a grant record cannot leak the workspace location.
    pub workspace: String,
    pub backend: ExecutionBackend,
    pub lifetime: GrantLifetime,
    pub state: GrantState,
}

impl Grant {
    /// Whether this grant, as it stands right now, authorizes `subject`
    /// on `workspace`/`backend`.
    ///
    /// Every clause is exact equality. There is no prefix, subset or
    /// "close enough" matching anywhere in this function, by design.
    pub fn covers(
        &self,
        subject: &PolicySubject,
        workspace: &str,
        backend: ExecutionBackend,
    ) -> bool {
        if self.state != GrantState::Issued {
            return false;
        }
        if self.workspace != workspace || self.backend != backend {
            return false;
        }
        subjects_equal(&self.subject, subject)
    }

    pub fn is_consumed(&self) -> bool {
        matches!(self.state, GrantState::Consumed { .. })
    }
}

/// Exact structural equality between two subjects.
///
/// Written out rather than relying on derived `PartialEq` only so the
/// version-binding rule is explicit: when the grant carries an observed
/// file version, the proposed subject must carry the *same* version. A
/// grant bound to a version does not cover an unversioned proposal, nor
/// one whose version moved.
fn subjects_equal(granted: &PolicySubject, proposed: &PolicySubject) -> bool {
    match (granted, proposed) {
        (PolicySubject::Filesystem(g), PolicySubject::Filesystem(p)) => {
            g.operation == p.operation
                && g.path == p.path
                && match (g.observed, p.observed) {
                    (Some(gv), Some(pv)) => gv == pv,
                    (None, None) => true,
                    // One side is version-bound and the other is not.
                    // Not the same operation as far as the grant is
                    // concerned.
                    _ => false,
                }
        }
        (PolicySubject::Shell(g), PolicySubject::Shell(p)) => g == p,
        (PolicySubject::Network(g), PolicySubject::Network(p)) => g == p,
        (PolicySubject::Git(g), PolicySubject::Git(p)) => g == p,
        (
            PolicySubject::Credential { reference: g },
            PolicySubject::Credential { reference: p },
        ) => g == p,
        (PolicySubject::Unknown { kind: g }, PolicySubject::Unknown { kind: p }) => g == p,
        _ => false,
    }
}

/// An explicit user denial.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Denial {
    pub id: String,
    pub scope: DenialScope,
}

/// What a denial covers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DenialScope {
    /// Everything of a kind. "Never run shells in this session."
    Kind(SubjectKind),
    /// One exact operation.
    Exact(Box<PolicySubject>),
}

impl Denial {
    pub fn matches(&self, subject: &PolicySubject) -> bool {
        match &self.scope {
            DenialScope::Kind(kind) => subject.kind() == *kind,
            DenialScope::Exact(denied) => subjects_equal(denied, subject),
        }
    }
}

/// The three results PRD 12 defines.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PolicyOutcome {
    Allow,
    Deny,
    AskUser,
}

/// Why a decision came out the way it did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PolicyReason {
    /// An explicit denial matched. Checked before anything that could
    /// permit the operation.
    DeniedByUser,
    /// The governing permission is `deny`.
    DeniedByPolicy {
        key: String,
        origin: PermissionOrigin,
    },
    /// A live grant covers this exact operation.
    GrantMatches { grant_id: String },
    /// The governing permission is `allow`.
    PolicyAllows {
        key: String,
        origin: PermissionOrigin,
    },
    /// The governing permission is `ask`.
    PolicyRequestsApproval {
        key: String,
        origin: PermissionOrigin,
    },
    /// No permission governs this subject, so it fails closed.
    NoGateFailsClosed,
    /// YOLO suppressed an `AskUser`. Attributed to the mode, never
    /// recorded as an individual user approval.
    YoloSuppressesApproval,
}

/// A policy decision with its reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub outcome: PolicyOutcome,
    pub reason: PolicyReason,
}

impl PolicyDecision {
    fn new(outcome: PolicyOutcome, reason: PolicyReason) -> Self {
        Self { outcome, reason }
    }

    pub fn allows(&self) -> bool {
        self.outcome == PolicyOutcome::Allow
    }
}

/// Evaluate one subject against one resolved permission, one denial and
/// one grant.
///
/// `workspace` and `backend` are the workspace fingerprint and backend
/// the operation actually targets. They are supplied by the caller
/// rather than read off the grant: a grant checked against itself would
/// satisfy its own workspace clause trivially and prove nothing.
///
/// The order is the contract:
///
/// 1. A matching denial wins over everything, including a live grant for
///    the exact same operation.
/// 2. A live grant covering the exact operation allows it.
/// 3. Otherwise the governing permission decides, and a subject with no
///    governing key fails closed to `AskUser`.
pub fn evaluate(
    subject: &PolicySubject,
    workspace: &str,
    backend: ExecutionBackend,
    permission: &ResolvedPermission,
    denial: Option<&Denial>,
    grant: Option<&Grant>,
) -> PolicyDecision {
    if denial.is_some_and(|denial| denial.matches(subject)) {
        return PolicyDecision::new(PolicyOutcome::Deny, PolicyReason::DeniedByUser);
    }

    if let Some(grant) = grant.filter(|grant| grant.covers(subject, workspace, backend)) {
        return PolicyDecision::new(
            PolicyOutcome::Allow,
            PolicyReason::GrantMatches {
                grant_id: grant.id.clone(),
            },
        );
    }

    match subject.policy_key() {
        None => PolicyDecision::new(PolicyOutcome::AskUser, PolicyReason::NoGateFailsClosed),
        Some(key) => match permission.value {
            Permission::Deny => PolicyDecision::new(
                PolicyOutcome::Deny,
                PolicyReason::DeniedByPolicy {
                    key: key.to_owned(),
                    origin: permission.origin,
                },
            ),
            Permission::Ask => PolicyDecision::new(
                PolicyOutcome::AskUser,
                PolicyReason::PolicyRequestsApproval {
                    key: key.to_owned(),
                    origin: permission.origin,
                },
            ),
            Permission::Allow => PolicyDecision::new(
                PolicyOutcome::Allow,
                PolicyReason::PolicyAllows {
                    key: key.to_owned(),
                    origin: permission.origin,
                },
            ),
        },
    }
}

/// Apply session YOLO to a decision.
///
/// PRD 12.2: YOLO makes operations that would otherwise `AskUser`
/// proceed. It changes nothing else. A `Deny` stays a `Deny` and
/// produces no prompt; an `Allow` is already allowed. The resulting
/// reason names YOLO so the audit trail attributes the authorization to
/// the mode rather than to a user who never approved this operation.
pub fn apply_yolo(decision: PolicyDecision, yolo: bool) -> PolicyDecision {
    if yolo && decision.outcome == PolicyOutcome::AskUser {
        return PolicyDecision::new(PolicyOutcome::Allow, PolicyReason::YoloSuppressesApproval);
    }
    decision
}
