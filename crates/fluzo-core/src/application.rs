use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_PAGE_SIZE: u32 = 64;
pub const MAX_TEXT_BYTES: usize = 8192;

macro_rules! identifier {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        pub struct $name(pub u64);
    )+};
}

identifier!(
    TaskId,
    SessionId,
    WorkspaceId,
    AttemptId,
    OperationId,
    RequestId,
    SubscriptionId
);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    pub epoch: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskState {
    Pending,
    Ready,
    Running,
    Waiting,
    Blocked,
    Stopping,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AttemptState {
    Pending,
    Running,
    Interrupted,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum OperationState {
    Proposed,
    AwaitingApproval,
    Dispatched,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WaitReason {
    WorkspaceOwnership,
    ModelCapacity,
    Permission,
    UserInput,
    Budget,
    Configuration,
    UncertainOutcome,
    TerminationUnconfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VerificationState {
    NotRun,
    Passed,
    Failed,
    Waived,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttemptSnapshot {
    pub id: AttemptId,
    pub state: AttemptState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub id: OperationId,
    pub version: u64,
    pub state: OperationState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub session: SessionId,
    pub workspace: WorkspaceId,
    pub version: u64,
    pub state: TaskState,
    pub reason: Option<WaitReason>,
    pub attempt: Option<AttemptSnapshot>,
    pub operation: Option<OperationSnapshot>,
    pub verification: VerificationState,
    pub active_milliseconds: u64,
    pub title: String,
}

impl TaskSnapshot {
    pub fn validate(&self) -> Result<(), ApplicationError> {
        if self.id.0 == 0
            || self.session.0 == 0
            || self.workspace.0 == 0
            || self.version == 0
            || self.title.len() > MAX_TEXT_BYTES
            || self
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.id.0 == 0)
            || self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.id.0 == 0 || operation.version == 0)
        {
            return Err(ApplicationError::InvalidRequest);
        }
        if matches!(
            self.state,
            TaskState::Waiting | TaskState::Blocked | TaskState::Stopping
        ) && self.reason.is_none()
        {
            return Err(ApplicationError::InvalidRequest);
        }
        if self.state == TaskState::Completed
            && (!matches!(
                self.verification,
                VerificationState::Passed | VerificationState::Waived
            ) || self
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.state != AttemptState::Completed)
                || self.operation.as_ref().is_some_and(|operation| {
                    matches!(
                        operation.state,
                        OperationState::Dispatched | OperationState::Unknown
                    )
                }))
        {
            return Err(ApplicationError::InvalidRequest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Command {
    Start {
        task: TaskId,
        session: SessionId,
        workspace: WorkspaceId,
        prompt: String,
    },
    Resume {
        task: TaskId,
        expected_version: u64,
    },
    Cancel {
        task: TaskId,
        expected_version: u64,
    },
    ForceStop {
        task: TaskId,
        expected_version: u64,
        confirmed: bool,
    },
    Approve {
        task: TaskId,
        expected_version: u64,
        operation: OperationId,
        operation_version: u64,
    },
}

impl Command {
    pub fn task(&self) -> TaskId {
        match self {
            Self::Start { task, .. }
            | Self::Resume { task, .. }
            | Self::Cancel { task, .. }
            | Self::ForceStop { task, .. }
            | Self::Approve { task, .. } => *task,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub protocol_version: u32,
    pub request: RequestId,
    pub command: Command,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Acknowledgement {
    pub request: RequestId,
    pub task: TaskId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum RequestStatus {
    Unknown,
    Accepted(Acknowledgement),
    Completed {
        acknowledgement: Acknowledgement,
        task_version: u64,
    },
    Failed {
        acknowledgement: Acknowledgement,
        error: ApplicationError,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageCursor {
    pub revision: Cursor,
    pub after: TaskId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum Query {
    Task {
        task: TaskId,
    },
    Tasks {
        after: Option<PageCursor>,
        limit: u32,
    },
    Request {
        request: RequestId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub protocol_version: u32,
    pub query: Query,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub protocol_version: u32,
    pub demo: bool,
    pub cursor: Cursor,
    pub tasks: Vec<TaskSnapshot>,
    pub next_page: Option<PageCursor>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum QueryResponse {
    Snapshot(Snapshot),
    Request(RequestStatus),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Update {
    pub cursor: Cursor,
    pub task: TaskSnapshot,
    pub request: Option<RequestId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateBatch {
    pub protocol_version: u32,
    pub demo: bool,
    pub updates: Vec<Update>,
    pub through: Cursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    pub protocol_version: u32,
    pub after: Cursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PollRequest {
    pub protocol_version: u32,
    pub subscription: SubscriptionId,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ApplicationError {
    ProtocolMismatch { supported: u32 },
    InvalidRequest,
    NotFound,
    StaleVersion { current: u64 },
    RequestConflict,
    ConfigurationRequired,
    PermissionRequired,
    PermissionDenied,
    ConfirmationRequired,
    BudgetExhausted,
    CapacityExhausted,
    CapacityWaitTimeout,
    Cancelled,
    UnknownOutcome,
    TransportUnavailable,
    AcknowledgementUnknown { request: RequestId },
    ResyncRequired { current: Cursor },
    HostStopped,
    UnexpectedCommand,
}

impl std::fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ApplicationError {}

pub trait ApplicationPort {
    fn command(&mut self, request: CommandRequest) -> Result<Acknowledgement, ApplicationError>;
    fn query(&self, request: QueryRequest) -> Result<QueryResponse, ApplicationError>;
    fn subscribe(&mut self, request: SubscribeRequest) -> Result<SubscriptionId, ApplicationError>;
    fn poll(&mut self, request: PollRequest) -> Result<UpdateBatch, ApplicationError>;
    fn detach(&mut self, subscription: SubscriptionId) -> Result<(), ApplicationError>;
}

pub fn check_protocol(version: u32) -> Result<(), ApplicationError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ApplicationError::ProtocolMismatch {
            supported: PROTOCOL_VERSION,
        })
    }
}

pub fn check_page_size(limit: u32) -> Result<(), ApplicationError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        Err(ApplicationError::InvalidRequest)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_and_page_limits_fail_explicitly() {
        assert!(check_protocol(PROTOCOL_VERSION).is_ok());
        assert_eq!(
            check_protocol(0),
            Err(ApplicationError::ProtocolMismatch { supported: 1 })
        );
        for limit in [0, MAX_PAGE_SIZE + 1, u32::MAX] {
            assert!(check_page_size(limit).is_err());
        }
        assert!(check_page_size(MAX_PAGE_SIZE).is_ok());
    }

    #[test]
    fn completed_requires_verification_and_no_unknown_operation() {
        let mut task = TaskSnapshot {
            id: TaskId(1),
            session: SessionId(1),
            workspace: WorkspaceId(1),
            version: 1,
            state: TaskState::Completed,
            reason: None,
            attempt: None,
            operation: None,
            verification: VerificationState::NotRun,
            active_milliseconds: 0,
            title: "fixture".to_owned(),
        };
        assert!(task.validate().is_err());
        task.verification = VerificationState::Passed;
        assert!(task.validate().is_ok());
        task.operation = Some(OperationSnapshot {
            id: OperationId(1),
            version: 1,
            state: OperationState::Unknown,
        });
        assert!(task.validate().is_err());
        task.state = TaskState::Stopping;
        assert!(task.validate().is_err());
        task.reason = Some(WaitReason::TerminationUnconfirmed);
        assert!(task.validate().is_ok());
    }
}
