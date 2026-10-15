//! UI-04 isolated demo states for tasks, permissions, diffs and task notices.
//!
//! Everything here is a projection of already-supplied synthetic protocol data
//! plus local view state. The module holds no application port, no clock-driven
//! side effect and no filesystem or network access, so no function in it can
//! resume, cancel, approve, stage or otherwise dispatch work. The demo records
//! what a control *would* do and labels the record as synthetic; the real
//! contracts live in the runtime and are out of scope for UI-04.
//!
//! Two structural rules carry most of the safety in this module:
//!
//! 1. Option lists put the non-destructive choice at index `0` and every view
//!    initialises its selection to `0`, so opening a confirmation or permission
//!    dialog cannot preselect an affirmative action (PRD 31).
//! 2. Every projected string passes through [`safe_text`], which visibly escapes
//!    control and bidirectional characters instead of dropping or forwarding them
//!    (PRD 29.2.3).

use fluzo_core::application::{Snapshot, TaskSnapshot, TaskState, VerificationState, WaitReason};

use crate::notification_stack::Severity;
use crate::setup::safe_text;

/// Characters that occupy no visible width but survive copy, paste and rendering.
/// They are escaped in security-sensitive projections so a path or command
/// cannot read differently from its underlying value (PRD 29.2.3).
const INVISIBLE: [char; 6] = [
    '\u{200b}', // zero width space
    '\u{200c}', // zero width non-joiner
    '\u{200d}', // zero width joiner
    '\u{2060}', // word joiner
    '\u{feff}', // byte order mark
    '\u{00ad}', // soft hyphen
];

/// [`safe_text`] plus visible escaping of zero-width characters. Used for every
/// projected task title, diff line and permission scope.
pub fn display(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if INVISIBLE.contains(&character) {
                character.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}

/// Sanitizes untrusted display content for the task, permission and diff views.
pub fn sanitize(text: &str) -> String {
    display(&safe_text(text))
}

/// Appended to projected values so a simulated result cannot read as a real one.
pub const SYNTHETIC: &str = "synthetic";

/// Stated wherever a demo control appears, so the absence of a runtime is explicit.
pub const NO_RUNTIME: &str = "no runtime connected";

/// Confirmation options. Index `0` is the non-destructive option and is always
/// the initial selection.
pub const CONFIRM_OPTIONS: [&str; 2] = ["Keep running", "Force stop anyway"];

/// Permission options. Index `0` is denial and is always the initial selection.
pub const PERMISSION_OPTIONS: [&str; 3] = ["Deny", "Allow once", "Allow for this task"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAction {
    Cancel,
    ForceStop,
}

impl TaskAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::ForceStop => "Force stop",
        }
    }

    /// Graceful cancel needs a deliberate row selection only. Force stop needs a
    /// separate confirmation that names the task, the processes and the risk of
    /// partial effects (PRD 17.2). The two are never the same control.
    pub fn requires_confirmation(self) -> bool {
        matches!(self, Self::ForceStop)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Confirmation {
    pub task: u64,
    pub action: TaskAction,
    pub selection: usize,
    pub processes: String,
    pub risk: String,
}

impl Confirmation {
    pub fn new(task: u64) -> Self {
        Self {
            task,
            action: TaskAction::ForceStop,
            selection: 0,
            processes: "synthetic process group 9001 (demo; no real process)".to_owned(),
            risk: "partial edits stay in the working tree; termination is unverified".to_owned(),
        }
    }

    pub fn destructive_selected(&self) -> bool {
        self.selection >= 1
    }

    pub fn move_selection(&mut self, forward: bool) {
        self.selection = if forward {
            (self.selection + 1).min(CONFIRM_OPTIONS.len() - 1)
        } else {
            self.selection.saturating_sub(1)
        };
    }

    /// Yields the action only when the destructive row was explicitly reached.
    /// With the default selection this returns `None`, so an accidental Enter
    /// keeps the task running.
    pub fn activate(&mut self) -> Option<TaskAction> {
        self.destructive_selected().then_some(self.action)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    Deny,
    AllowOnce,
    AllowForTask,
}

/// Which class of authority a pending permission request is asking for. The
/// demo carries this on the row so a single session can show more than one
/// scope. It is display metadata only: it authorizes nothing and consumes no
/// grant. `Credential` requests reference `$FLUZO_DEMO_TOKEN`; `WorkspaceWrite`
/// requests name a workspace path and carry no credential reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionScope {
    Credential,
    WorkspaceWrite,
}

impl PermissionDecision {
    /// Any unexpected index resolves to denial: an ambiguous input must never
    /// become an approval.
    pub fn from_index(index: usize) -> Self {
        match index {
            1 => Self::AllowOnce,
            2 => Self::AllowForTask,
            _ => Self::Deny,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Deny => "Denied",
            Self::AllowOnce => "Allowed once",
            Self::AllowForTask => "Allowed for this task",
        }
    }

    pub fn is_approval(self) -> bool {
        !matches!(self, Self::Deny)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionPrompt {
    pub task: u64,
    pub operation: u64,
    pub action: String,
    pub workspace: u64,
    pub backend: String,
    pub scope: String,
    pub selection: usize,
    pub history: Vec<(String, PermissionDecision)>,
}

impl PermissionPrompt {
    /// A sanitized synthetic approval request. Sensitive values are shown as
    /// references rather than values, matching the configuration rule that the
    /// UI never displays secrets.
    pub fn synthetic(task: u64, operation: u64, workspace: u64) -> Self {
        Self {
            task,
            operation,
            action: sanitize("tool.write_file"),
            workspace,
            backend: sanitize("native local workspace (demo)"),
            scope: sanitize(
                "src/fixture.rs [observed version v7] | env: $FLUZO_DEMO_TOKEN (reference only)",
            ),
            selection: 0,
            history: Vec::new(),
        }
    }

    /// A workspace-write permission request with no credential reference.
    /// Names a workspace path and an observed file version only. Synthetic:
    /// it never opens the path and never authorizes the write.
    pub fn workspace_write(task: u64, operation: u64, workspace: u64) -> Self {
        Self {
            task,
            operation,
            action: sanitize("tool.write_file"),
            workspace,
            backend: sanitize("native local workspace (demo)"),
            scope: sanitize(
                "src/fixture.rs [observed version v7] | workspace-write (no credential)",
            ),
            selection: 0,
            history: Vec::new(),
        }
    }

    pub fn decision(&self) -> PermissionDecision {
        PermissionDecision::from_index(self.selection)
    }

    pub fn move_selection(&mut self, forward: bool) {
        self.selection = if forward {
            (self.selection + 1).min(PERMISSION_OPTIONS.len() - 1)
        } else {
            self.selection.saturating_sub(1)
        };
    }

    /// Records the decision against the displayed action. Denials win and stay in
    /// history; recording is not dispatch, so nothing is consumed or released.
    pub fn decide(&mut self) -> PermissionDecision {
        let decision = self.decision();
        self.history
            .push((format!("{} #{}", self.action, self.operation), decision));
        decision
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeOrigin {
    Agent,
    PreExistingUser,
    Unknown,
}

impl ChangeOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::PreExistingUser => "pre-existing user change",
            Self::Unknown => "origin unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffLine {
    pub origin: ChangeOrigin,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffView {
    pub lines: Vec<DiffLine>,
    pub verification: VerificationState,
    pub partial: bool,
}

/// Stated on every diff review: looking is not acting (PRD 30).
pub const REVIEW_NOTICE: &str = "Reviewing does not stage, revert, commit or accept any change.";

impl DiffView {
    pub fn synthetic(verification: VerificationState, partial: bool) -> Self {
        let mut lines = vec![
            DiffLine {
                origin: ChangeOrigin::Agent,
                text: "-let width = 80;".to_owned(),
            },
            DiffLine {
                origin: ChangeOrigin::Agent,
                text: "+let width = viewport.width;".to_owned(),
            },
            DiffLine {
                origin: ChangeOrigin::PreExistingUser,
                text: "-// user note: keep the old default".to_owned(),
            },
            DiffLine {
                origin: ChangeOrigin::Unknown,
                text: "+// attribution unavailable for this hunk".to_owned(),
            },
        ];
        if partial {
            lines.push(DiffLine {
                origin: ChangeOrigin::Agent,
                text: "+// partial: cancelled before this hunk completed".to_owned(),
            });
        }
        for line in &mut lines {
            line.text = sanitize(&line.text);
        }
        Self {
            lines,
            verification,
            partial,
        }
    }

    pub fn verification_label(&self) -> &'static str {
        match self.verification {
            VerificationState::NotRun => "verification not run",
            VerificationState::Passed => "verification passed",
            VerificationState::Failed => "verification failed",
            VerificationState::Waived => "verification waived",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRow {
    pub id: u64,
    pub title: String,
    pub session: u64,
    pub workspace: u64,
    pub state: TaskState,
    pub reason: Option<WaitReason>,
    pub action: String,
    pub budget: String,
    pub interventions: usize,
    pub verification: VerificationState,
    pub permission_scope: PermissionScope,
}

impl TaskRow {
    pub fn state_label(&self) -> &'static str {
        state_label(self.state)
    }
}

pub fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Ready => "ready",
        TaskState::Running => "running",
        TaskState::Waiting => "waiting",
        TaskState::Blocked => "blocked",
        TaskState::Stopping => "stopping",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::Cancelled => "cancelled",
    }
}

pub fn reason_label(reason: WaitReason) -> &'static str {
    match reason {
        WaitReason::WorkspaceOwnership => "workspace ownership",
        WaitReason::ModelCapacity => "model capacity",
        WaitReason::Permission => "permission",
        WaitReason::UserInput => "user input",
        WaitReason::Budget => "budget",
        WaitReason::Configuration => "configuration",
        WaitReason::UncertainOutcome => "uncertain outcome",
        WaitReason::TerminationUnconfirmed => "termination unconfirmed",
    }
}

fn current_action(task: &TaskSnapshot) -> &'static str {
    use fluzo_core::application::OperationState;
    match (&task.operation, task.state) {
        (Some(operation), _) => match operation.state {
            OperationState::Proposed => "operation proposed",
            OperationState::AwaitingApproval => "awaiting approval",
            OperationState::Dispatched => "operation dispatched",
            OperationState::Succeeded => "operation succeeded",
            OperationState::Failed => "operation failed",
            OperationState::Cancelled => "operation cancelled",
            OperationState::Unknown => "outcome unknown",
        },
        (None, TaskState::Stopping) => "graceful stop requested",
        (None, TaskState::Waiting) => "waiting for input",
        (None, TaskState::Blocked) => "held by policy",
        (None, _) => "no operation",
    }
}

fn budget_text(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!("{:>2}m{:02}s active", seconds / 60, seconds % 60)
}

/// Projects the snapshot tasks into display rows. Read-only: the returned rows
/// are owned copies and the snapshot is not mutated.
pub fn project(snapshot: &Snapshot) -> Vec<TaskRow> {
    snapshot
        .tasks
        .iter()
        .map(|task| TaskRow {
            id: task.id.0,
            title: sanitize(&task.title),
            session: task.session.0,
            workspace: task.workspace.0,
            state: task.state,
            reason: task.reason,
            action: current_action(task).to_owned(),
            budget: budget_text(task.active_milliseconds),
            interventions: usize::from(task.state == TaskState::Waiting),
            verification: task.verification,
            permission_scope: PermissionScope::Credential,
        })
        .collect()
}

/// The multi-state Ctrl+T review fixture: one session showing the full task
/// state space, two permission scopes, and partial diffs with differing
/// verification. Deterministic and synthetic: no runtime, provider, inference
/// or real workspace access. Built by projecting real [`TaskSnapshot`] values
/// so the projection and sanitization path is exercised, then annotating the
/// permission scopes the protocol snapshot itself does not carry.
pub fn multi_state_fixture() -> TaskDemo {
    use fluzo_core::application::{
        AttemptId, AttemptSnapshot, AttemptState, Cursor, SessionId, TaskId, TaskSnapshot,
        WorkspaceId,
    };
    let task = |id: u64,
                state: TaskState,
                reason: Option<WaitReason>,
                attempt: AttemptState,
                verification: VerificationState,
                title: &str| TaskSnapshot {
        id: TaskId(id),
        session: SessionId(1),
        workspace: WorkspaceId(7),
        version: 1,
        state,
        reason,
        attempt: Some(AttemptSnapshot {
            id: AttemptId(id),
            state: attempt,
        }),
        operation: None,
        verification,
        active_milliseconds: 81_000,
        title: title.to_owned(),
    };
    let snapshot = Snapshot {
        protocol_version: 1,
        demo: true,
        cursor: Cursor {
            epoch: 1,
            sequence: 0,
        },
        tasks: vec![
            task(
                101,
                TaskState::Pending,
                None,
                AttemptState::Pending,
                VerificationState::NotRun,
                "queued: index repository symbols",
            ),
            task(
                102,
                TaskState::Running,
                None,
                AttemptState::Running,
                VerificationState::NotRun,
                "running: refactor composer keymap",
            ),
            task(
                103,
                TaskState::Blocked,
                Some(WaitReason::Permission),
                AttemptState::Running,
                VerificationState::NotRun,
                "blocked: credential needed to fetch model catalog",
            ),
            task(
                104,
                TaskState::Blocked,
                Some(WaitReason::Permission),
                AttemptState::Running,
                VerificationState::NotRun,
                "blocked: workspace-write needed for src/fixture.rs",
            ),
            task(
                105,
                TaskState::Completed,
                None,
                AttemptState::Completed,
                VerificationState::Passed,
                "completed: add regression test",
            ),
            task(
                106,
                TaskState::Failed,
                None,
                AttemptState::Failed,
                VerificationState::Failed,
                "failed: migrate storage layer",
            ),
            task(
                107,
                TaskState::Stopping,
                Some(WaitReason::TerminationUnconfirmed),
                AttemptState::Running,
                VerificationState::NotRun,
                "stopping: cancel long-running build",
            ),
            task(
                108,
                TaskState::Cancelled,
                None,
                AttemptState::Cancelled,
                VerificationState::NotRun,
                "cancelled: revert partial edit",
            ),
        ],
        next_page: None,
    };
    let mut rows = project(&snapshot);
    for row in &mut rows {
        if row.id == 104 {
            row.permission_scope = PermissionScope::WorkspaceWrite;
        }
    }
    TaskDemo::with_rows(rows)
}

/// Task-severity mapping for synthetic notices. Completion, blocked-on-input and
/// failure are eligible; quiet states raise nothing.
pub fn notice_severity(state: TaskState) -> Option<Severity> {
    match state {
        TaskState::Blocked => Some(Severity::Approval),
        TaskState::Failed => Some(Severity::Error),
        TaskState::Completed => Some(Severity::Success),
        TaskState::Stopping => Some(Severity::Warning),
        _ => None,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskDemo {
    pub rows: Vec<TaskRow>,
    pub selection: usize,
    pub detail: bool,
    pub confirmation: Option<Confirmation>,
    pub permission: Option<PermissionPrompt>,
    pub diff: Option<DiffView>,
    pub outcome: Option<String>,
    pub permission_history: Vec<(String, PermissionDecision)>,
    cancel_requests: usize,
    inspections: usize,
}

impl TaskDemo {
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            rows: project(snapshot),
            ..Self::default()
        }
    }

    /// Builds a demo state from already-projected rows, keeping the counter
    /// fields private to this module.
    pub fn with_rows(rows: Vec<TaskRow>) -> Self {
        Self {
            rows,
            ..Self::default()
        }
    }

    pub fn selected(&self) -> Option<&TaskRow> {
        self.rows.get(self.selection)
    }

    pub fn move_selection(&mut self, forward: bool) {
        if self.rows.is_empty() {
            return;
        }
        self.selection = if forward {
            (self.selection + 1).min(self.rows.len() - 1)
        } else {
            self.selection.saturating_sub(1)
        };
    }

    /// Opens the detail of the selected task. This is ordinary history
    /// inspection: it changes no task state, takes no writer ownership, changes
    /// no grant and redirects no input (PRD 17.1).
    pub fn open_detail(&mut self) {
        let Some(id) = self.selected().map(|row| row.id) else {
            return;
        };
        self.inspections += 1;
        self.detail = true;
        self.outcome = Some(format!("Inspecting task {id} (synthetic; {NO_RUNTIME})"));
    }

    pub fn close_detail(&mut self) {
        self.detail = false;
    }

    /// True while a nested dialog owns the keyboard, so Esc closes that dialog
    /// instead of the whole view.
    pub fn nested(&self) -> bool {
        self.confirmation.is_some() || self.permission.is_some() || self.diff.is_some()
    }

    pub fn request_cancel(&mut self) {
        let Some(id) = self.selected().map(|row| row.id) else {
            return;
        };
        self.cancel_requests += 1;
        self.outcome = Some(format!(
            "Synthetic graceful cancel recorded for task {id} ({}; request #{}, nothing dispatched)",
            NO_RUNTIME, self.cancel_requests
        ));
    }

    pub fn request_force_stop(&mut self) {
        if let Some(id) = self.selected().map(|row| row.id) {
            self.confirmation = Some(Confirmation::new(id));
        }
    }

    pub fn resolve_confirmation(&mut self) {
        let Some(mut confirmation) = self.confirmation.take() else {
            return;
        };
        self.outcome = Some(match confirmation.activate() {
            Some(action) => format!(
                "Synthetic {} confirmed for task {} ({}; nothing dispatched)",
                action.label(),
                confirmation.task,
                NO_RUNTIME
            ),
            None => format!(
                "Force stop dismissed; task {} keeps running (synthetic)",
                confirmation.task
            ),
        });
    }

    pub fn outcome(&self) -> Option<&str> {
        self.outcome.as_deref()
    }

    pub fn open_permission(&mut self) {
        if let Some(row) = self.selected() {
            let (id, workspace, scope) = (row.id, row.workspace, row.permission_scope);
            self.permission = Some(match scope {
                PermissionScope::Credential => {
                    PermissionPrompt::synthetic(id, id.saturating_add(7000), workspace)
                }
                PermissionScope::WorkspaceWrite => {
                    PermissionPrompt::workspace_write(id, id.saturating_add(7000), workspace)
                }
            });
        }
    }

    pub fn resolve_permission(&mut self) {
        let Some(mut prompt) = self.permission.take() else {
            return;
        };
        let decision = prompt.decide();
        self.outcome = Some(format!(
            "Synthetic permission {} for task {} ({}; no grant consumed)",
            decision.label(),
            prompt.task,
            NO_RUNTIME
        ));
        self.permission_history = prompt.history;
    }

    pub fn open_diff(&mut self) {
        if let Some((verification, partial)) = self.selected().map(|row| {
            (
                row.verification,
                matches!(row.state, TaskState::Cancelled | TaskState::Failed),
            )
        }) {
            self.diff = Some(DiffView::synthetic(verification, partial));
        }
    }

    pub fn inspections(&self) -> usize {
        self.inspections
    }

    pub fn cancel_requests(&self) -> usize {
        self.cancel_requests
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluzo_core::application::{
        AttemptSnapshot, AttemptState, Cursor, OperationSnapshot, OperationState, SessionId,
        TaskId, WorkspaceId,
    };

    fn task(id: u64, state: TaskState, reason: Option<WaitReason>) -> TaskSnapshot {
        TaskSnapshot {
            id: TaskId(id),
            session: SessionId(1),
            workspace: WorkspaceId(7),
            version: 1,
            state,
            reason,
            attempt: Some(AttemptSnapshot {
                id: fluzo_core::application::AttemptId(id),
                state: AttemptState::Running,
            }),
            operation: None,
            verification: VerificationState::NotRun,
            active_milliseconds: 81_000,
            title: format!("Synthetic task {id}"),
        }
    }

    fn snapshot(tasks: Vec<TaskSnapshot>) -> Snapshot {
        Snapshot {
            protocol_version: fluzo_core::application::PROTOCOL_VERSION,
            demo: true,
            cursor: Cursor {
                epoch: 1,
                sequence: 1,
            },
            tasks,
            next_page: None,
        }
    }

    #[test]
    fn projection_is_read_only_and_labels_every_field() {
        let source = snapshot(vec![
            task(1, TaskState::Running, None),
            task(2, TaskState::Blocked, Some(WaitReason::WorkspaceOwnership)),
        ]);
        let rows = project(&source);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].state_label(), "running");
        assert_eq!(rows[0].budget, " 1m21s active");
        assert_eq!(
            rows[1].reason.map(reason_label),
            Some("workspace ownership")
        );
        assert!(rows.iter().all(|row| row.title.contains("Synthetic")));
        // The snapshot itself is untouched by projection.
        assert_eq!(source.tasks[0].state, TaskState::Running);
        assert_eq!(source.tasks[1].reason, Some(WaitReason::WorkspaceOwnership));
    }

    #[test]
    fn projection_escapes_control_and_bidirectional_content_in_titles() {
        let mut hostile = task(3, TaskState::Waiting, Some(WaitReason::UserInput));
        hostile.title =
            "fix \u{202e}src/payroll.rs\u{202c} \u{1b}]0;pwned\u{7} and \u{200b}more".to_owned();
        let rows = project(&snapshot(vec![hostile]));
        let title = &rows[0].title;
        assert!(!title.contains('\u{1b}'), "{title:?}");
        assert!(!title.contains('\u{202e}'), "{title:?}");
        assert!(!title.contains('\u{202c}'), "{title:?}");
        // The underlying value stays recoverable rather than silently dropped.
        assert!(title.contains("\\u{202e}"), "{title:?}");
        assert!(title.contains("\\u{200b}"), "{title:?}");
    }

    #[test]
    fn cancel_and_force_stop_are_distinct_controls() {
        assert!(!TaskAction::Cancel.requires_confirmation());
        assert!(TaskAction::ForceStop.requires_confirmation());
        assert_ne!(TaskAction::Cancel.label(), TaskAction::ForceStop.label());
    }

    #[test]
    fn confirmation_defaults_to_the_non_destructive_option() {
        let confirmation = Confirmation::new(4);
        assert_eq!(confirmation.selection, 0);
        assert_eq!(CONFIRM_OPTIONS[0], "Keep running");
        assert!(!confirmation.destructive_selected());
    }

    #[test]
    fn force_stop_requires_an_explicit_move_to_the_destructive_row() {
        let mut confirmation = Confirmation::new(4);
        // Default selection: activating keeps the task running.
        assert_eq!(confirmation.activate(), None);
        confirmation.move_selection(true);
        assert!(confirmation.destructive_selected());
        assert_eq!(confirmation.activate(), Some(TaskAction::ForceStop));
        // The safe row remains reachable; selection is not sticky.
        confirmation.move_selection(false);
        assert_eq!(confirmation.activate(), None);
    }

    #[test]
    fn permission_prompt_never_preselects_an_approval() {
        let prompt = PermissionPrompt::synthetic(2, 7002, 7);
        assert_eq!(prompt.selection, 0);
        assert_eq!(PERMISSION_OPTIONS[0], "Deny");
        assert!(!prompt.decision().is_approval());
        assert!(!PermissionDecision::from_index(99).is_approval());
        assert!(!PermissionDecision::from_index(usize::MAX).is_approval());
    }

    #[test]
    fn permission_decision_is_recorded_without_dispatch() {
        let mut prompt = PermissionPrompt::synthetic(2, 7002, 7);
        assert_eq!(prompt.decide(), PermissionDecision::Deny);
        prompt.move_selection(true);
        assert_eq!(prompt.decide(), PermissionDecision::AllowOnce);
        prompt.move_selection(true);
        assert_eq!(prompt.decide(), PermissionDecision::AllowForTask);
        assert_eq!(prompt.history.len(), 3);
        assert_eq!(prompt.history[0].1, PermissionDecision::Deny);
        assert!(prompt.history[0].0.contains("tool.write_file"));
    }

    #[test]
    fn permission_scope_shows_references_not_values() {
        let prompt = PermissionPrompt::synthetic(2, 7002, 7);
        assert!(prompt.scope.contains("$FLUZO_DEMO_TOKEN"));
        assert!(prompt.scope.contains("reference only"));
        assert!(prompt.scope.contains("observed version v7"));
    }

    #[test]
    fn diff_review_states_that_it_acts_on_nothing() {
        assert!(REVIEW_NOTICE.contains("stage"));
        assert!(REVIEW_NOTICE.contains("revert"));
        assert!(REVIEW_NOTICE.contains("commit"));
        let view = DiffView::synthetic(VerificationState::Failed, false);
        assert_eq!(view.verification_label(), "verification failed");
        assert!(
            view.lines
                .iter()
                .any(|line| line.origin == ChangeOrigin::PreExistingUser),
            "pre-existing user attribution required"
        );
        assert!(
            view.lines
                .iter()
                .any(|line| line.origin == ChangeOrigin::Unknown),
            "unknown origin must be shown, not guessed"
        );
    }

    #[test]
    fn cancelled_diff_is_marked_partial() {
        let full = DiffView::synthetic(VerificationState::NotRun, false);
        let partial = DiffView::synthetic(VerificationState::NotRun, true);
        assert_eq!(full.lines.len(), 4);
        assert_eq!(partial.lines.len(), 5);
        assert!(
            partial.lines[4]
                .text
                .contains("cancelled before this hunk completed")
        );
    }

    #[test]
    fn diff_content_is_sanitized() {
        let mut view = DiffView::synthetic(VerificationState::NotRun, false);
        view.lines[0].text = "+rm -rf \u{1b}[31m\u{202e}/\u{202c}".to_owned();
        for line in &mut view.lines {
            line.text = sanitize(&line.text);
        }
        assert!(view.lines.iter().all(|line| !line.text.contains('\u{1b}')));
        assert!(view.lines[0].text.contains("\\u{1b}"));
    }

    #[test]
    fn notice_severity_covers_the_eligible_states_only() {
        assert_eq!(
            notice_severity(TaskState::Blocked),
            Some(Severity::Approval)
        );
        assert_eq!(notice_severity(TaskState::Failed), Some(Severity::Error));
        assert_eq!(
            notice_severity(TaskState::Completed),
            Some(Severity::Success)
        );
        assert_eq!(
            notice_severity(TaskState::Stopping),
            Some(Severity::Warning)
        );
        assert_eq!(notice_severity(TaskState::Pending), None);
        assert_eq!(notice_severity(TaskState::Running), None);
        assert_eq!(notice_severity(TaskState::Cancelled), None);
    }

    #[test]
    fn selecting_a_task_does_not_resume_it() {
        let mut demo = TaskDemo::new(&snapshot(vec![
            task(1, TaskState::Pending, None),
            task(2, TaskState::Running, None),
            task(3, TaskState::Blocked, Some(WaitReason::Permission)),
        ]));
        assert_eq!(demo.selection, 0);
        demo.move_selection(true);
        demo.move_selection(true);
        assert_eq!(demo.selection, 2);
        assert_eq!(demo.selected().unwrap().id, 3);
        demo.open_detail();
        // Inspection is counted and nothing else moved.
        assert_eq!(demo.inspections(), 1);
        assert_eq!(demo.rows[2].state, TaskState::Blocked);
        assert_eq!(demo.rows[2].reason, Some(WaitReason::Permission));
        // The only recorded effect is the inspection itself: no cancel, no
        // force stop, no permission decision and no state transition.
        let outcome = demo.outcome().unwrap();
        assert!(outcome.starts_with("Inspecting task 3"), "{outcome}");
        assert!(demo.confirmation.is_none());
        assert!(demo.permission.is_none());
        assert!(demo.diff.is_none());
        assert_eq!(demo.cancel_requests(), 0);
        assert_eq!(demo.permission_history.len(), 0);
        assert!(demo.detail);
    }

    #[test]
    fn selection_clamps_at_both_ends_without_wrapping() {
        let mut demo = TaskDemo::new(&snapshot(vec![task(1, TaskState::Pending, None)]));
        demo.move_selection(false);
        assert_eq!(demo.selection, 0);
        demo.move_selection(true);
        demo.move_selection(true);
        assert_eq!(demo.selection, 0);
    }

    #[test]
    fn repeated_cancellation_is_idempotent_and_never_escalates() {
        let mut demo = TaskDemo::new(&snapshot(vec![task(1, TaskState::Running, None)]));
        demo.open_detail();
        demo.request_cancel();
        let first = demo.outcome().map(str::to_owned);
        demo.request_cancel();
        assert_eq!(demo.cancel_requests(), 2);
        assert_ne!(demo.outcome().unwrap(), first.as_deref().unwrap());
        assert!(demo.outcome().unwrap().contains("request #2"));
        // Cancelling never opens the force-stop confirmation by itself.
        assert!(demo.confirmation.is_none());
        assert_eq!(demo.rows[0].state, TaskState::Running);
    }

    #[test]
    fn force_stop_always_passes_through_the_confirmation() {
        let mut demo = TaskDemo::new(&snapshot(vec![task(1, TaskState::Running, None)]));
        demo.open_detail();
        demo.request_force_stop();
        assert!(demo.confirmation.is_some());
        assert!(demo.nested());
        // Dismissing at the default selection keeps the task running.
        demo.resolve_confirmation();
        assert!(demo.outcome().unwrap().contains("keeps running"));
        assert!(demo.confirmation.is_none());
        demo.request_force_stop();
        if let Some(confirmation) = &mut demo.confirmation {
            confirmation.move_selection(true);
        }
        demo.resolve_confirmation();
        let outcome = demo.outcome().unwrap().to_owned();
        assert!(outcome.contains("Force stop confirmed"), "{outcome}");
        assert!(outcome.contains("nothing dispatched"), "{outcome}");
    }

    #[test]
    fn every_demo_outcome_names_the_missing_runtime() {
        let mut demo = TaskDemo::new(&snapshot(vec![task(1, TaskState::Running, None)]));
        demo.open_detail();
        assert!(demo.outcome().unwrap().contains(NO_RUNTIME));
        demo.request_cancel();
        assert!(demo.outcome().unwrap().contains(NO_RUNTIME));
        demo.request_force_stop();
        if let Some(c) = &mut demo.confirmation {
            c.move_selection(true);
        }
        demo.resolve_confirmation();
        assert!(demo.outcome().unwrap().contains(NO_RUNTIME));
        demo.open_permission();
        demo.resolve_permission();
        assert!(demo.outcome().unwrap().contains(NO_RUNTIME));
    }

    #[test]
    fn empty_snapshot_yields_no_rows_and_no_panic() {
        let mut demo = TaskDemo::new(&snapshot(Vec::new()));
        assert!(demo.rows.is_empty());
        demo.move_selection(true);
        demo.open_detail();
        demo.request_cancel();
        demo.request_force_stop();
        demo.open_permission();
        demo.open_diff();
        demo.resolve_confirmation();
        demo.resolve_permission();
        assert!(demo.outcome().is_none());
        assert!(!demo.nested());
    }

    #[test]
    fn operation_state_drives_the_current_action_label() {
        let mut running = task(1, TaskState::Running, None);
        running.operation = Some(OperationSnapshot {
            id: fluzo_core::application::OperationId(9),
            version: 1,
            state: OperationState::AwaitingApproval,
        });
        let rows = project(&snapshot(vec![running]));
        assert_eq!(rows[0].action, "awaiting approval");
        let mut unknown = task(
            2,
            TaskState::Stopping,
            Some(WaitReason::TerminationUnconfirmed),
        );
        unknown.operation = Some(OperationSnapshot {
            id: fluzo_core::application::OperationId(10),
            version: 1,
            state: OperationState::Unknown,
        });
        let rows = project(&snapshot(vec![unknown]));
        assert_eq!(rows[0].action, "outcome unknown");
        assert_eq!(
            rows[0].reason.map(reason_label),
            Some("termination unconfirmed")
        );
    }

    fn fixture() -> TaskDemo {
        multi_state_fixture()
    }

    fn select(demo: &mut TaskDemo, id: u64) {
        let index = demo.rows.iter().position(|row| row.id == id).unwrap();
        demo.selection = index;
    }

    #[test]
    fn fixture_shows_every_required_state_in_one_session() {
        let demo = fixture();
        assert!(demo.rows.len() >= 5, "fixture needs 5+ tasks");
        let states: Vec<TaskState> = demo.rows.iter().map(|row| row.state).collect();
        for required in [
            TaskState::Pending,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Completed,
            TaskState::Failed,
            TaskState::Stopping,
        ] {
            assert!(states.contains(&required), "missing state {required:?}");
        }
        let blocked = demo
            .rows
            .iter()
            .filter(|row| {
                row.state == TaskState::Blocked && row.reason == Some(WaitReason::Permission)
            })
            .count();
        assert!(blocked >= 2, "need 2+ blocked-awaiting-permission tasks");
    }

    #[test]
    fn fixture_permission_requests_have_two_distinct_scopes() {
        let mut demo = fixture();

        select(&mut demo, 103);
        demo.open_permission();
        let credential = demo.permission.take().unwrap();
        assert!(credential.scope.contains("$FLUZO_DEMO_TOKEN"));
        assert!(credential.scope.contains("reference only"));
        assert!(
            !credential.scope.contains("workspace-write"),
            "credential request must not be a workspace-write request"
        );

        select(&mut demo, 104);
        demo.open_permission();
        let write = demo.permission.take().unwrap();
        assert!(write.scope.contains("workspace-write"));
        assert!(write.scope.contains("no credential"));
        assert!(
            !write.scope.contains("$FLUZO_DEMO_TOKEN"),
            "workspace-write request must not reference a credential"
        );
        assert!(write.scope.contains("src/fixture.rs"));
    }

    #[test]
    fn fixture_has_two_partial_diffs_with_differing_verification() {
        let mut demo = fixture();

        select(&mut demo, 106);
        demo.open_diff();
        let failed = demo.diff.take().unwrap();
        assert!(failed.partial, "failed task diff is partial");
        assert_eq!(failed.verification, VerificationState::Failed);

        select(&mut demo, 108);
        demo.open_diff();
        let cancelled = demo.diff.take().unwrap();
        assert!(cancelled.partial, "cancelled task diff is partial");
        assert_eq!(cancelled.verification, VerificationState::NotRun);

        assert_ne!(
            failed.verification, cancelled.verification,
            "the two partial diffs must differ in verification"
        );

        select(&mut demo, 105);
        demo.open_diff();
        let completed = demo.diff.take().unwrap();
        assert!(!completed.partial);
        assert_eq!(completed.verification, VerificationState::Passed);
    }

    #[test]
    fn fixture_safety_invariants_hold() {
        let mut demo = fixture();

        // Selection never resumes work: opening detail is inspection only.
        select(&mut demo, 103);
        demo.open_detail();
        assert_eq!(demo.inspections(), 1);
        assert!(
            !demo.outcome().unwrap_or("").contains("resumed"),
            "selecting a task must not resume it"
        );

        // Deny is the first permission choice on both scopes.
        select(&mut demo, 104);
        demo.open_permission();
        assert_eq!(
            demo.permission.as_ref().unwrap().decision(),
            PermissionDecision::Deny
        );
        // Approvals require confirmation; denial does not.
        demo.permission.as_mut().unwrap().selection = 1;
        assert!(demo.permission.as_ref().unwrap().decision().is_approval());
        // Resolving records the decision without dispatching or consuming a grant.
        demo.resolve_permission();
        assert!(demo.permission.is_none());
        assert!(demo.outcome().unwrap().contains("no grant consumed"));

        // Reviewing a diff never stages, commits or pushes.
        select(&mut demo, 106);
        demo.open_diff();
        let diff = demo.diff.as_ref().unwrap();
        assert!(REVIEW_NOTICE.contains("stage") && REVIEW_NOTICE.contains("commit"));
        let joined: Vec<&str> = diff.lines.iter().map(|line| line.text.as_str()).collect();
        let joined = joined.join("\n");
        assert!(!joined.contains("staged"));
        assert!(!joined.contains("pushed"));
    }

    #[test]
    fn fixture_is_deterministic() {
        assert_eq!(multi_state_fixture(), multi_state_fixture());
    }
}
