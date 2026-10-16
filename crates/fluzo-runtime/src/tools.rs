//! Tool dispatch: the seam between a proposal and the machinery that may run it.
//!
//! RUN-03c (PRD 13, 17.2, 21.3; Architecture A05). The process layer runs
//! programs it owns, the jail decides where a path may land, and the ledger
//! decides whether this exact operation is authorized. This module is where
//! those three meet, and the order they are consulted in is the contract.
//!
//! **Nothing runs before the decision.** A denied request returns the denial
//! reason rather than a prompt, and no process is created. A request that needs
//! approval in a host that cannot provide one returns the RUN-02 unavailable
//! outcome. Both happen before any spawn.
//!
//! **The approval covers the thing that actually runs.** The dispatcher derives
//! the scope of the argv, shell mode, working directory and environment
//! overrides that would be handed to the child, and compares it with the scope
//! the caller says was approved. Any mismatch is refused before anything runs.
//! An approval that describes one program while another runs is worse than no
//! approval.
//!
//! **The grant is spent before the side effect.** Consumption precedes the
//! spawn, so a second dispatch of the same grant is refused rather than racing
//! with the first. If the effect then becomes unconfirmed the grant is marked
//! uncertain and stays spent; nothing re-arms it.
//!
//! **A refusal never turns into a different attempt.** A jail refusal is
//! returned as a jail refusal. A failed direct spawn is returned as a spawn
//! failure. There is no path through this module that reaches a shell because a
//! native attempt was rejected, and no retry of any kind.
//!
//! This seam has no production caller yet: there is no agent loop to generate
//! requests (AGT chain) and no budget admission in front of it (#19).

use std::path::PathBuf;
use std::time::Duration;

use fluzo_core::policy::{
    ExecutionBackend, FileOperation, FileScope, PolicyReason, PolicySubject, ResolvedPermission,
    ShellMode, ShellScope,
};
use fluzo_core::settings::Permission;

use crate::authorization::LedgerError;
use crate::confinement::{ConfinementError, WorkspaceJail};
use crate::git_inspect::{self, Inspection, InspectionError};
use crate::process::{
    self, EnvironmentSpec, ProcessOutcome, ProcessRequest, SignalRefusal, SpawnConfig, SpawnError,
    TerminationMode,
};
use crate::session_auth::{ApprovalRequest, AuthOutcome, AuthorizationService};

/// Capture and timing ceilings for one dispatched command.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub capture_limit: usize,
    pub timeout: Duration,
    pub grace: Duration,
    pub escalation: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            capture_limit: process::DEFAULT_CAPTURE_LIMIT,
            timeout: Duration::from_secs(30),
            grace: process::DEFAULT_GRACE_PERIOD,
            escalation: process::DEFAULT_ESCALATION_WINDOW,
        }
    }
}

/// A command the caller wants run.
#[derive(Clone, Debug)]
pub struct CommandTool {
    pub dispatch_id: String,
    pub approval_id: String,
    /// Workspace-relative directory the child runs in. The jail is the root.
    pub working_directory: String,
    /// The scope the caller says was approved. Compared against the derived
    /// scope of what will actually run; a mismatch is refused.
    pub approved: ShellScope,
    pub request: ProcessRequest,
    pub environment: EnvironmentSpec,
    pub limits: Limits,
}

/// A proposed tool action.
#[derive(Clone, Debug)]
pub enum ToolRequest {
    Command(Box<CommandTool>),
    ReadFile {
        dispatch_id: String,
        approval_id: String,
        path: String,
    },
    WriteFile {
        dispatch_id: String,
        approval_id: String,
        path: String,
        contents: Vec<u8>,
    },
    InspectGit {
        dispatch_id: String,
        approval_id: String,
        inspection: Inspection,
    },
}

/// What actually happened to an authorized request.
#[derive(Debug)]
pub enum ExecutionResult {
    Process(ProcessOutcome),
    Contents(String),
    Written { path: String, bytes: usize },
}

/// The dispatch answer.
#[derive(Debug)]
pub enum ToolOutcome {
    /// Authorized and run.
    Executed(ExecutionResult),
    /// Refused by policy. Producing this created no process and wrote nothing.
    Denied { reason: PolicyReason },
    /// An interactive host should prompt. Nothing has run yet.
    ApprovalRequired(ApprovalRequest),
    /// Approval was needed and this host cannot provide it.
    ApprovalUnavailable(ApprovalRequest),
}

impl ToolOutcome {
    pub fn executed(&self) -> bool {
        matches!(self, Self::Executed(_))
    }
}

/// Why a dispatch failed rather than being refused by policy.
#[derive(Debug, PartialEq, Eq)]
pub enum ToolError {
    /// The approved scope does not describe the request that was submitted.
    /// Nothing ran and no grant was consumed.
    SubjectMismatch { detail: String },
    /// The jail refused the path. The request was not retried any other way.
    Confinement(ConfinementError),
    /// The process could not be spawned.
    Spawn(SpawnError),
    /// The owned group could not be safely signalled.
    Signal(SignalRefusal),
    /// The grant could not be consumed.
    Ledger(LedgerError),
    /// The inspection was rejected before anything ran.
    Inspection(InspectionError),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubjectMismatch { detail } => {
                write!(
                    formatter,
                    "approved scope does not match the request: {detail}"
                )
            }
            Self::Confinement(error) => write!(formatter, "refused by the workspace jail: {error}"),
            Self::Spawn(error) => write!(formatter, "could not start the tool: {error}"),
            Self::Signal(error) => write!(formatter, "could not cancel the tool: {error}"),
            Self::Ledger(error) => write!(formatter, "grant could not be consumed: {error}"),
            Self::Inspection(error) => write!(formatter, "inspection rejected: {error}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// Supplies the governing permission for a policy key.
///
/// The dispatcher does not read configuration itself. It asks for the resolved
/// permission of the key that governs the subject in front of it, so the
/// origin-aware resolution rules stay in the configuration layer and this seam
/// consumes decisions rather than producing them.
pub trait PermissionSource {
    fn permission_for(&self, key: &str) -> ResolvedPermission;
}

/// A source that answers every key the same way.
///
/// Useful for tests and for hosts that have not wired a settings registry in
/// front of the dispatcher yet.
pub struct ConstantPermission(pub ResolvedPermission);

impl PermissionSource for ConstantPermission {
    fn permission_for(&self, _key: &str) -> ResolvedPermission {
        self.0
    }
}

/// The permission used where no policy key governs the subject.
///
/// `Ask` with no gate means only a live grant or a denial can move the decision;
/// the dispatcher never infers an allowance of its own.
const NO_GATE: ResolvedPermission = ResolvedPermission {
    value: Permission::Ask,
    origin: fluzo_core::policy::PermissionOrigin::BuiltIn,
    ignored_repository_broaden: None,
};

/// Dispatches tool requests through the policy layer, the grant ledger and the
/// workspace jail.
pub struct ToolDispatcher<'a> {
    auth: &'a mut AuthorizationService,
    jail: &'a WorkspaceJail,
    root: PathBuf,
    workspace: String,
    permissions: &'a dyn PermissionSource,
}

impl<'a> ToolDispatcher<'a> {
    pub fn new(
        auth: &'a mut AuthorizationService,
        jail: &'a WorkspaceJail,
        root: impl Into<PathBuf>,
        workspace: impl Into<String>,
        permissions: &'a dyn PermissionSource,
    ) -> Self {
        Self {
            auth,
            jail,
            root: root.into(),
            workspace: workspace.into(),
            permissions,
        }
    }

    /// Dispatch one request.
    ///
    /// The order is fixed: derive the real scope, compare it with the approved
    /// scope, decide, consume, then act. A refusal at any step returns without
    /// having performed the later ones.
    ///
    /// # Errors
    ///
    /// Returns a [`ToolError`] when the request cannot be carried out. Policy
    /// refusals are *not* errors: they come back as [`ToolOutcome`] values.
    pub fn dispatch(&mut self, request: &ToolRequest) -> Result<ToolOutcome, ToolError> {
        match request {
            ToolRequest::Command(tool) => self.dispatch_command(tool),
            ToolRequest::ReadFile {
                dispatch_id,
                approval_id,
                path,
            } => self.dispatch_file(dispatch_id, approval_id, FileOperation::Read, path, None),
            ToolRequest::WriteFile {
                dispatch_id,
                approval_id,
                path,
                contents,
            } => self.dispatch_file(
                dispatch_id,
                approval_id,
                FileOperation::Write,
                path,
                Some(contents),
            ),
            ToolRequest::InspectGit {
                dispatch_id,
                approval_id,
                inspection,
            } => self.dispatch_inspection(dispatch_id, approval_id, inspection),
        }
    }

    fn dispatch_command(&mut self, tool: &CommandTool) -> Result<ToolOutcome, ToolError> {
        let actual = actual_scope(&tool.request, &tool.environment, &tool.working_directory);
        if tool.approved != actual {
            return Err(ToolError::SubjectMismatch {
                detail: format!(
                    "approved {} but the request would run {}",
                    describe_scope(&tool.approved),
                    describe_scope(&actual)
                ),
            });
        }

        let subject = PolicySubject::Shell(actual);
        let backend = backend_of(&tool.request);
        let reason = match self.authorize(&tool.approval_id, &subject, backend) {
            AuthOutcome::Allowed { reason } => reason,
            other => return Ok(refusal_of(&other)),
        };

        // Spend the grant before the side effect. A grant that cannot be spent
        // means this dispatch does not happen at all.
        self.consume(&reason, &tool.dispatch_id)?;

        let config = SpawnConfig::new(
            tool.request.clone(),
            self.root.join(&tool.working_directory),
            tool.environment.clone(),
        )
        .with_capture_limit(tool.limits.capture_limit);
        let outcome = self.run(config, &reason, tool.limits)?;
        Ok(ToolOutcome::Executed(ExecutionResult::Process(outcome)))
    }

    fn dispatch_file(
        &mut self,
        dispatch_id: &str,
        approval_id: &str,
        operation: FileOperation,
        path: &str,
        contents: Option<&[u8]>,
    ) -> Result<ToolOutcome, ToolError> {
        if operation == FileOperation::Write && contents.is_none() {
            return Err(ToolError::SubjectMismatch {
                detail: "a write request must carry contents".to_string(),
            });
        }

        // Filesystem access has no permission knob: the RUN-01 jail is the
        // authority, so only a live grant or a denial moves this decision.
        let subject = PolicySubject::Filesystem(FileScope::new(operation, path));
        let reason = match self.authorize(approval_id, &subject, ExecutionBackend::NativeLocal) {
            AuthOutcome::Allowed { reason } => reason,
            other => return Ok(refusal_of(&other)),
        };
        self.consume(&reason, dispatch_id)?;

        // The jail is consulted after the decision, and its refusal ends the
        // dispatch. Nothing downstream converts it into another attempt.
        match (operation, contents) {
            (FileOperation::Read, _) => {
                let mut file = self.jail.open_read(path).map_err(ToolError::Confinement)?;
                let text = file.read_to_string().map_err(ToolError::Confinement)?;
                Ok(ToolOutcome::Executed(ExecutionResult::Contents(text)))
            }
            (FileOperation::Write, Some(bytes)) => {
                let mut file = self.jail.open_write(path).map_err(ToolError::Confinement)?;
                file.replace_all(bytes).map_err(ToolError::Confinement)?;
                file.sync_all().map_err(ToolError::Confinement)?;
                Ok(ToolOutcome::Executed(ExecutionResult::Written {
                    path: path.to_string(),
                    bytes: bytes.len(),
                }))
            }
            (FileOperation::Write, None) => unreachable!("checked above"),
            _ => Err(ToolError::SubjectMismatch {
                detail: format!("{operation:?} is not a supported file tool"),
            }),
        }
    }

    fn dispatch_inspection(
        &mut self,
        dispatch_id: &str,
        approval_id: &str,
        inspection: &Inspection,
    ) -> Result<ToolOutcome, ToolError> {
        let arguments =
            git_inspect::inspection_arguments(inspection).map_err(ToolError::Inspection)?;
        let environment = git_inspect::hardened_git_environment();
        let request = ProcessRequest::direct("git", arguments);
        let actual = actual_scope(&request, &environment, ".");
        let subject = PolicySubject::Shell(actual);
        let reason = match self.authorize(approval_id, &subject, ExecutionBackend::NativeLocal) {
            AuthOutcome::Allowed { reason } => reason,
            other => return Ok(refusal_of(&other)),
        };
        self.consume(&reason, dispatch_id)?;

        let config = SpawnConfig::new(request, self.root.clone(), environment)
            .with_capture_limit(git_inspect::DEFAULT_INSPECTION_LIMIT);
        let outcome = self.run(
            config,
            &reason,
            Limits {
                capture_limit: git_inspect::DEFAULT_INSPECTION_LIMIT,
                timeout: Duration::from_secs(30),
                grace: Duration::from_secs(2),
                escalation: Duration::from_secs(2),
            },
        )?;
        Ok(ToolOutcome::Executed(ExecutionResult::Process(outcome)))
    }

    /// Runs the decided action and records uncertainty against the grant.
    fn run(
        &mut self,
        config: SpawnConfig,
        reason: &PolicyReason,
        limits: Limits,
    ) -> Result<ProcessOutcome, ToolError> {
        let mut process = process::spawn(config).map_err(ToolError::Spawn)?;
        let outcome = process
            .run_to_completion(limits.timeout, limits.grace, limits.escalation)
            .map_err(ToolError::Signal)?;
        if outcome.termination == TerminationMode::Uncertain {
            self.mark_uncertain(reason);
        }
        Ok(outcome)
    }

    /// Resolves the decision for one subject without running anything.
    ///
    /// Returns the session service's own outcome so a denial, a prompt need and
    /// a headless unavailability stay distinguishable to the caller.
    fn authorize(
        &self,
        approval_id: &str,
        subject: &PolicySubject,
        backend: ExecutionBackend,
    ) -> AuthOutcome {
        let permission = subject
            .policy_key()
            .map(|key| self.permissions.permission_for(key))
            .unwrap_or(NO_GATE);
        self.auth
            .resolve(approval_id, subject, &self.workspace, backend, &permission)
    }

    fn consume(&mut self, reason: &PolicyReason, dispatch_id: &str) -> Result<(), ToolError> {
        if let PolicyReason::GrantMatches { grant_id } = reason {
            self.auth
                .ledger_mut()
                .consume(grant_id, dispatch_id)
                .map_err(ToolError::Ledger)?;
        }
        Ok(())
    }

    fn mark_uncertain(&mut self, reason: &PolicyReason) {
        if let PolicyReason::GrantMatches { grant_id } = reason {
            let _ = self.auth.ledger_mut().mark_outcome_uncertain(grant_id);
        }
    }
}

/// Maps a non-allowing session outcome onto the dispatch outcome.
fn refusal_of(outcome: &AuthOutcome) -> ToolOutcome {
    match outcome {
        AuthOutcome::Denied { reason } => ToolOutcome::Denied {
            reason: reason.clone(),
        },
        AuthOutcome::ApprovalRequired(request) => ToolOutcome::ApprovalRequired(request.clone()),
        AuthOutcome::ApprovalUnavailable(request) => {
            ToolOutcome::ApprovalUnavailable(request.clone())
        }
        AuthOutcome::Allowed { reason } => ToolOutcome::Denied {
            reason: reason.clone(),
        },
    }
}

/// A diagnostic description of a scope that does not print environment values.
///
/// Overrides may carry credentials, and a mismatch message is destined for a
/// log or a UI. The shape is enough to explain the refusal.
fn describe_scope(scope: &ShellScope) -> String {
    format!(
        "{} {}/{} (cwd {cwd}, {env_count} env override(s))",
        match scope.mode {
            ShellMode::Direct => "direct",
            ShellMode::ExplicitShell => "explicit shell",
        },
        scope.executable,
        scope.arguments.join(" "),
        cwd = scope.working_directory,
        env_count = scope.environment.len(),
    )
}

fn backend_of(request: &ProcessRequest) -> ExecutionBackend {
    match request {
        ProcessRequest::Direct { .. } => ExecutionBackend::NativeLocal,
        ProcessRequest::ExplicitShell { .. } => ExecutionBackend::ExplicitShell,
    }
}

/// Derives the scope of what a request would actually run.
///
/// This is the value the approval has to match. It is built from the request
/// itself rather than from a description of it, which is what makes the
/// comparison meaningful.
fn actual_scope(
    request: &ProcessRequest,
    environment: &EnvironmentSpec,
    working_directory: &str,
) -> ShellScope {
    let (mode, executable, arguments) = match request {
        ProcessRequest::Direct {
            executable,
            arguments,
        } => (ShellMode::Direct, executable.clone(), arguments.clone()),
        ProcessRequest::ExplicitShell { shell, script } => (
            ShellMode::ExplicitShell,
            shell.clone(),
            vec!["-c".to_string(), script.clone()],
        ),
    };
    ShellScope {
        mode,
        executable,
        arguments,
        working_directory: working_directory.to_string(),
        environment: environment.overrides(),
    }
}
