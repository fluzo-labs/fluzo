use fluzo_core::application::*;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScenarioLimits {
    pub tasks: usize,
    pub steps: usize,
    pub requests: usize,
    pub subscriptions: usize,
    pub retained_updates: usize,
}

impl Default for ScenarioLimits {
    fn default() -> Self {
        Self {
            tasks: 64,
            steps: 128,
            requests: 128,
            subscriptions: 8,
            retained_updates: 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioStep {
    pub expected: Command,
    pub result: Result<TaskSnapshot, ApplicationError>,
    pub lose_acknowledgement: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostState {
    Running,
    Quiescing,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownReport {
    pub unresolved_requests: Vec<RequestId>,
    pub unresolved_tasks: Vec<TaskId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScenarioEvidence {
    pub accepted: usize,
    pub pending: usize,
    pub remaining: usize,
    pub unexpected: usize,
}

#[derive(Clone)]
struct RecordedRequest {
    command: Command,
    status: RequestStatus,
}

pub struct ScenarioDriver {
    limits: ScenarioLimits,
    cursor: Cursor,
    tasks: BTreeMap<TaskId, TaskSnapshot>,
    steps: VecDeque<ScenarioStep>,
    pending: VecDeque<(RequestId, ScenarioStep)>,
    requests: BTreeMap<RequestId, RecordedRequest>,
    subscriptions: BTreeMap<SubscriptionId, Cursor>,
    updates: VecDeque<Update>,
    next_subscription: u64,
    unexpected: usize,
    host: HostState,
}

impl ScenarioDriver {
    pub fn new(
        epoch: u64,
        initial: Vec<TaskSnapshot>,
        steps: Vec<ScenarioStep>,
        limits: ScenarioLimits,
    ) -> Result<Self, ApplicationError> {
        if epoch == 0
            || [
                limits.tasks,
                limits.steps,
                limits.requests,
                limits.subscriptions,
                limits.retained_updates,
            ]
            .contains(&0)
            || limits.tasks > 4096
            || limits.steps > 4096
            || limits.requests > 4096
            || limits.subscriptions > 4096
            || limits.retained_updates > 4096
            || initial.len() > limits.tasks
            || steps.len() > limits.steps
        {
            return Err(ApplicationError::InvalidRequest);
        }
        let mut tasks = BTreeMap::new();
        for task in initial {
            task.validate()?;
            if tasks.insert(task.id, task).is_some() {
                return Err(ApplicationError::InvalidRequest);
            }
        }
        for step in &steps {
            validate_command(&step.expected)?;
            if let Ok(task) = &step.result {
                task.validate()?;
                if step.expected.task() != task.id {
                    return Err(ApplicationError::InvalidRequest);
                }
                if let Command::Start {
                    session, workspace, ..
                } = &step.expected
                    && (task.session != *session || task.workspace != *workspace)
                {
                    return Err(ApplicationError::InvalidRequest);
                }
            }
        }
        Ok(Self {
            limits,
            cursor: Cursor { epoch, sequence: 0 },
            tasks,
            steps: steps.into(),
            pending: VecDeque::new(),
            requests: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            updates: VecDeque::new(),
            next_subscription: 1,
            unexpected: 0,
            host: HostState::Running,
        })
    }

    pub fn evidence(&self) -> ScenarioEvidence {
        ScenarioEvidence {
            accepted: self.requests.len(),
            pending: self.pending.len(),
            remaining: self.steps.len(),
            unexpected: self.unexpected,
        }
    }

    pub fn verify_complete(&self) -> Result<(), ApplicationError> {
        if !self.steps.is_empty() || !self.pending.is_empty() || self.unexpected != 0 {
            Err(ApplicationError::UnexpectedCommand)
        } else {
            Ok(())
        }
    }

    pub fn host_state(&self) -> HostState {
        self.host
    }

    pub fn quiesce(&mut self) {
        if self.host == HostState::Running {
            self.host = HostState::Quiescing;
        }
    }

    pub fn shutdown(&mut self) -> ShutdownReport {
        self.host = HostState::Stopped;
        let unresolved_requests = self.pending.iter().map(|(request, _)| *request).collect();
        let unresolved_tasks = self
            .tasks
            .values()
            .filter(|task| {
                matches!(
                    task.state,
                    TaskState::Running | TaskState::Waiting | TaskState::Stopping
                ) || task.operation.as_ref().is_some_and(|operation| {
                    matches!(
                        operation.state,
                        OperationState::Dispatched | OperationState::Unknown
                    )
                })
            })
            .map(|task| task.id)
            .collect();
        self.subscriptions.clear();
        ShutdownReport {
            unresolved_requests,
            unresolved_tasks,
        }
    }

    pub fn advance(&mut self) -> Result<bool, ApplicationError> {
        if self.host == HostState::Stopped {
            return Err(ApplicationError::HostStopped);
        }
        let Some((request, step)) = self.pending.front().cloned() else {
            return Ok(false);
        };
        let acknowledgement = Acknowledgement {
            request,
            task: step.expected.task(),
        };
        let status = match step.result {
            Ok(task) => {
                let previous = self.tasks.get(&task.id);
                if previous.is_some_and(|previous| {
                    task.version <= previous.version
                        || task.session != previous.session
                        || task.workspace != previous.workspace
                        || task.active_milliseconds < previous.active_milliseconds
                }) {
                    return Err(ApplicationError::InvalidRequest);
                }
                if previous.is_none() && self.tasks.len() >= self.limits.tasks {
                    return Err(ApplicationError::CapacityExhausted);
                }
                let sequence = self
                    .cursor
                    .sequence
                    .checked_add(1)
                    .ok_or(ApplicationError::CapacityExhausted)?;
                self.cursor.sequence = sequence;
                self.tasks.insert(task.id, task.clone());
                if self.updates.len() == self.limits.retained_updates {
                    self.updates.pop_front();
                }
                let task_version = task.version;
                self.updates.push_back(Update {
                    cursor: self.cursor,
                    task,
                    request: Some(request),
                });
                RequestStatus::Completed {
                    acknowledgement,
                    task_version,
                }
            }
            Err(error) => RequestStatus::Failed {
                acknowledgement,
                error,
            },
        };
        if let Some(recorded) = self.requests.get_mut(&request) {
            recorded.status = status;
        }
        self.pending.pop_front();
        Ok(true)
    }

    fn check_cursor(&self, cursor: Cursor) -> Result<(), ApplicationError> {
        let oldest = self
            .updates
            .front()
            .map_or(self.cursor.sequence, |update| update.cursor.sequence - 1);
        if cursor.epoch != self.cursor.epoch
            || cursor.sequence > self.cursor.sequence
            || cursor.sequence < oldest
        {
            Err(ApplicationError::ResyncRequired {
                current: self.cursor,
            })
        } else {
            Ok(())
        }
    }

    fn check_command_state(&self, command: &Command) -> Result<(), ApplicationError> {
        if self
            .pending
            .iter()
            .any(|(_, step)| step.expected.task() == command.task())
        {
            return Err(ApplicationError::CapacityExhausted);
        }
        match command {
            Command::Start { task, .. } => {
                if self.tasks.contains_key(task) {
                    return Err(ApplicationError::RequestConflict);
                }
                let reserved = self
                    .pending
                    .iter()
                    .filter(|(_, step)| matches!(step.expected, Command::Start { .. }))
                    .count();
                if self.tasks.len() + reserved >= self.limits.tasks {
                    return Err(ApplicationError::CapacityExhausted);
                }
            }
            Command::Resume {
                task,
                expected_version,
            }
            | Command::Cancel {
                task,
                expected_version,
            }
            | Command::ForceStop {
                task,
                expected_version,
                ..
            }
            | Command::Approve {
                task,
                expected_version,
                ..
            } => {
                let current = self.tasks.get(task).ok_or(ApplicationError::NotFound)?;
                if *expected_version != current.version {
                    return Err(ApplicationError::StaleVersion {
                        current: current.version,
                    });
                }
                if let Command::Approve {
                    operation,
                    operation_version,
                    ..
                } = command
                {
                    let current = current
                        .operation
                        .as_ref()
                        .ok_or(ApplicationError::NotFound)?;
                    if current.id != *operation {
                        return Err(ApplicationError::NotFound);
                    }
                    if current.version != *operation_version {
                        return Err(ApplicationError::StaleVersion {
                            current: current.version,
                        });
                    }
                    if current.state != OperationState::AwaitingApproval {
                        return Err(ApplicationError::PermissionDenied);
                    }
                }
                if matches!(
                    command,
                    Command::ForceStop {
                        confirmed: false,
                        ..
                    }
                ) {
                    return Err(ApplicationError::ConfirmationRequired);
                }
            }
        }
        Ok(())
    }
}

impl ApplicationPort for ScenarioDriver {
    fn command(&mut self, request: CommandRequest) -> Result<Acknowledgement, ApplicationError> {
        check_protocol(request.protocol_version)?;
        if request.request.0 == 0 {
            return Err(ApplicationError::InvalidRequest);
        }
        validate_command(&request.command)?;
        if let Some(recorded) = self.requests.get(&request.request) {
            if recorded.command != request.command {
                return Err(ApplicationError::RequestConflict);
            }
            return Ok(Acknowledgement {
                request: request.request,
                task: request.command.task(),
            });
        }
        if self.host == HostState::Stopped {
            return Err(ApplicationError::HostStopped);
        }
        if self.host == HostState::Quiescing
            && !matches!(
                request.command,
                Command::Cancel { .. } | Command::ForceStop { .. }
            )
        {
            return Err(ApplicationError::HostStopped);
        }
        if self.requests.len() >= self.limits.requests {
            return Err(ApplicationError::CapacityExhausted);
        }
        self.check_command_state(&request.command)?;
        let Some(step) = self.steps.front() else {
            self.unexpected = self.unexpected.saturating_add(1);
            return Err(ApplicationError::UnexpectedCommand);
        };
        if step.expected != request.command {
            self.unexpected = self.unexpected.saturating_add(1);
            return Err(ApplicationError::UnexpectedCommand);
        }
        if let Ok(task) = &step.result
            && self.tasks.get(&task.id).is_some_and(|previous| {
                task.version <= previous.version
                    || task.session != previous.session
                    || task.workspace != previous.workspace
                    || task.active_milliseconds < previous.active_milliseconds
            })
        {
            return Err(ApplicationError::InvalidRequest);
        }
        let step = step.clone();
        self.steps.pop_front();
        let acknowledgement = Acknowledgement {
            request: request.request,
            task: request.command.task(),
        };
        self.requests.insert(
            request.request,
            RecordedRequest {
                command: request.command,
                status: RequestStatus::Accepted(acknowledgement),
            },
        );
        let lost = step.lose_acknowledgement;
        self.pending.push_back((request.request, step));
        if lost {
            Err(ApplicationError::AcknowledgementUnknown {
                request: request.request,
            })
        } else {
            Ok(acknowledgement)
        }
    }

    fn query(&self, request: QueryRequest) -> Result<QueryResponse, ApplicationError> {
        check_protocol(request.protocol_version)?;
        match request.query {
            Query::Request { request } => Ok(QueryResponse::Request(
                self.requests
                    .get(&request)
                    .map_or(RequestStatus::Unknown, |recorded| recorded.status.clone()),
            )),
            Query::Task { task } => {
                let task = self
                    .tasks
                    .get(&task)
                    .ok_or(ApplicationError::NotFound)?
                    .clone();
                Ok(QueryResponse::Snapshot(Snapshot {
                    protocol_version: PROTOCOL_VERSION,
                    demo: true,
                    cursor: self.cursor,
                    tasks: vec![task],
                    next_page: None,
                }))
            }
            Query::Tasks { after, limit } => {
                check_page_size(limit)?;
                if after.is_some_and(|after| after.revision != self.cursor) {
                    return Err(ApplicationError::ResyncRequired {
                        current: self.cursor,
                    });
                }
                let mut tasks: Vec<_> = self
                    .tasks
                    .values()
                    .filter(|task| after.is_none_or(|after| task.id > after.after))
                    .take(limit as usize + 1)
                    .cloned()
                    .collect();
                let next_page = if tasks.len() > limit as usize {
                    tasks.pop();
                    tasks.last().map(|task| PageCursor {
                        revision: self.cursor,
                        after: task.id,
                    })
                } else {
                    None
                };
                Ok(QueryResponse::Snapshot(Snapshot {
                    protocol_version: PROTOCOL_VERSION,
                    demo: true,
                    cursor: self.cursor,
                    tasks,
                    next_page,
                }))
            }
        }
    }

    fn subscribe(&mut self, request: SubscribeRequest) -> Result<SubscriptionId, ApplicationError> {
        check_protocol(request.protocol_version)?;
        if self.host == HostState::Stopped {
            return Err(ApplicationError::HostStopped);
        }
        self.check_cursor(request.after)?;
        if self.subscriptions.len() >= self.limits.subscriptions {
            return Err(ApplicationError::CapacityExhausted);
        }
        let id = SubscriptionId(self.next_subscription);
        self.next_subscription = self
            .next_subscription
            .checked_add(1)
            .ok_or(ApplicationError::CapacityExhausted)?;
        self.subscriptions.insert(id, request.after);
        Ok(id)
    }

    fn poll(&mut self, request: PollRequest) -> Result<UpdateBatch, ApplicationError> {
        check_protocol(request.protocol_version)?;
        check_page_size(request.limit)?;
        let cursor = *self
            .subscriptions
            .get(&request.subscription)
            .ok_or(ApplicationError::NotFound)?;
        self.check_cursor(cursor)?;
        let updates: Vec<_> = self
            .updates
            .iter()
            .filter(|update| update.cursor.sequence > cursor.sequence)
            .take(request.limit as usize)
            .cloned()
            .collect();
        let through = updates.last().map_or(cursor, |update| update.cursor);
        self.subscriptions.insert(request.subscription, through);
        Ok(UpdateBatch {
            protocol_version: PROTOCOL_VERSION,
            demo: true,
            updates,
            through,
        })
    }

    fn detach(&mut self, subscription: SubscriptionId) -> Result<(), ApplicationError> {
        if self.subscriptions.remove(&subscription).is_some() {
            Ok(())
        } else {
            Err(ApplicationError::NotFound)
        }
    }
}

fn validate_command(command: &Command) -> Result<(), ApplicationError> {
    if command.task().0 == 0 {
        return Err(ApplicationError::InvalidRequest);
    }
    match command {
        Command::Start {
            session,
            workspace,
            prompt,
            ..
        } if session.0 == 0
            || workspace.0 == 0
            || prompt.trim().is_empty()
            || prompt.len() > MAX_TEXT_BYTES =>
        {
            Err(ApplicationError::InvalidRequest)
        }
        Command::Resume {
            expected_version: 0,
            ..
        }
        | Command::Cancel {
            expected_version: 0,
            ..
        }
        | Command::ForceStop {
            expected_version: 0,
            ..
        }
        | Command::Approve {
            expected_version: 0,
            ..
        } => Err(ApplicationError::InvalidRequest),
        Command::Approve {
            operation,
            operation_version,
            ..
        } if operation.0 == 0 || *operation_version == 0 => Err(ApplicationError::InvalidRequest),
        _ => Ok(()),
    }
}

pub fn demo_driver() -> Result<ScenarioDriver, ApplicationError> {
    ScenarioDriver::new(
        1,
        vec![TaskSnapshot {
            id: TaskId(1),
            session: SessionId(1),
            workspace: WorkspaceId(1),
            version: 1,
            state: TaskState::Pending,
            reason: None,
            attempt: None,
            operation: None,
            verification: VerificationState::NotRun,
            active_milliseconds: 0,
            title: "Demo fixture; no agent or tools executed".to_owned(),
        }],
        Vec::new(),
        ScenarioLimits::default(),
    )
}
