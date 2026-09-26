use fluzo_core::application::{
    ApplicationError, ApplicationPort, PROTOCOL_VERSION, Query, QueryRequest, QueryResponse,
    Snapshot, TaskId,
};

pub fn inspect_task(
    port: &dyn ApplicationPort,
    task: TaskId,
) -> Result<Snapshot, ApplicationError> {
    match port.query(QueryRequest {
        protocol_version: PROTOCOL_VERSION,
        query: Query::Task { task },
    })? {
        QueryResponse::Snapshot(snapshot) if snapshot.protocol_version == PROTOCOL_VERSION => {
            Ok(snapshot)
        }
        QueryResponse::Snapshot(snapshot) => Err(ApplicationError::ProtocolMismatch {
            supported: snapshot.protocol_version,
        }),
        QueryResponse::Request(_) => Err(ApplicationError::InvalidRequest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fluzo_core::application::*;

    struct ReadOnlyFixture {
        snapshot: Snapshot,
    }

    impl ApplicationPort for ReadOnlyFixture {
        fn command(&mut self, _: CommandRequest) -> Result<Acknowledgement, ApplicationError> {
            panic!("inspection must not submit commands")
        }

        fn query(&self, request: QueryRequest) -> Result<QueryResponse, ApplicationError> {
            assert_eq!(request.protocol_version, PROTOCOL_VERSION);
            assert_eq!(request.query, Query::Task { task: TaskId(1) });
            Ok(QueryResponse::Snapshot(self.snapshot.clone()))
        }

        fn subscribe(&mut self, _: SubscribeRequest) -> Result<SubscriptionId, ApplicationError> {
            panic!("inspection must not attach implicitly")
        }

        fn poll(&mut self, _: PollRequest) -> Result<UpdateBatch, ApplicationError> {
            panic!("inspection must not poll implicitly")
        }

        fn detach(&mut self, _: SubscriptionId) -> Result<(), ApplicationError> {
            panic!("inspection must not own lifecycle")
        }
    }

    #[test]
    fn selection_only_reads_owned_snapshots_without_runtime_imports() {
        let fixture = ReadOnlyFixture {
            snapshot: Snapshot {
                protocol_version: PROTOCOL_VERSION,
                demo: true,
                cursor: Cursor {
                    epoch: 1,
                    sequence: 0,
                },
                tasks: vec![TaskSnapshot {
                    id: TaskId(1),
                    session: SessionId(1),
                    workspace: WorkspaceId(1),
                    version: 1,
                    state: TaskState::Blocked,
                    reason: Some(WaitReason::Permission),
                    attempt: None,
                    operation: None,
                    verification: VerificationState::NotRun,
                    active_milliseconds: 0,
                    title: "Fixture".to_owned(),
                }],
                next_page: None,
            },
        };
        let mut snapshot = inspect_task(&fixture, TaskId(1)).unwrap();
        snapshot.tasks[0].title.clear();
        assert_eq!(
            inspect_task(&fixture, TaskId(1)).unwrap().tasks[0].title,
            "Fixture"
        );
    }
}
