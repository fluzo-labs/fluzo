use crate::scenario::*;
use fluzo_core::application::*;

fn task(state: TaskState, version: u64) -> TaskSnapshot {
    TaskSnapshot {
        id: TaskId(1),
        session: SessionId(1),
        workspace: WorkspaceId(1),
        version,
        state,
        reason: None,
        attempt: None,
        operation: None,
        verification: VerificationState::NotRun,
        active_milliseconds: version,
        title: "Synthetic task".to_owned(),
    }
}

fn start() -> Command {
    Command::Start {
        task: TaskId(1),
        session: SessionId(1),
        workspace: WorkspaceId(1),
        prompt: "Synthetic task".to_owned(),
    }
}

fn request(id: u64, command: Command) -> CommandRequest {
    CommandRequest {
        protocol_version: PROTOCOL_VERSION,
        request: RequestId(id),
        command,
    }
}

fn query(query: Query) -> QueryRequest {
    QueryRequest {
        protocol_version: PROTOCOL_VERSION,
        query,
    }
}

fn step(expected: Command, result: TaskSnapshot) -> ScenarioStep {
    ScenarioStep {
        expected,
        result: Ok(result),
        lose_acknowledgement: false,
    }
}

fn list(port: &dyn ApplicationPort) -> Snapshot {
    let QueryResponse::Snapshot(snapshot) = port
        .query(query(Query::Tasks {
            after: None,
            limit: MAX_PAGE_SIZE,
        }))
        .unwrap()
    else {
        panic!("expected snapshot");
    };
    snapshot
}

fn poll(
    port: &mut dyn ApplicationPort,
    subscription: SubscriptionId,
) -> Result<UpdateBatch, ApplicationError> {
    port.poll(PollRequest {
        protocol_version: PROTOCOL_VERSION,
        subscription,
        limit: MAX_PAGE_SIZE,
    })
}

fn subscribe(port: &mut dyn ApplicationPort, after: Cursor) -> SubscriptionId {
    port.subscribe(SubscribeRequest {
        protocol_version: PROTOCOL_VERSION,
        after,
    })
    .unwrap()
}

fn inspection_contract(port: &mut dyn ApplicationPort) {
    let before = list(port);
    assert!(before.demo);
    let subscription = subscribe(port, before.cursor);
    assert!(poll(port, subscription).unwrap().updates.is_empty());
    port.detach(subscription).unwrap();
    assert_eq!(list(port), before);
    let reattached = subscribe(port, before.cursor);
    assert!(poll(port, reattached).unwrap().updates.is_empty());
    port.detach(reattached).unwrap();
    assert_eq!(list(port), before);
}

#[test]
fn inspection_and_reconnection_never_consume_a_script_or_execute_work() {
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![step(start(), task(TaskState::Running, 1))],
        ScenarioLimits::default(),
    )
    .unwrap();
    let before = driver.evidence();
    inspection_contract(&mut driver);
    assert_eq!(driver.evidence(), before);
    assert!(driver.verify_complete().is_err());
    let mut demo = demo_driver().unwrap();
    inspection_contract(&mut demo);
    assert!(demo.verify_complete().is_ok());
}

#[test]
fn lost_acknowledgement_is_reconciled_without_duplicate_dispatch() {
    let mut expected = step(start(), task(TaskState::Running, 1));
    expected.lose_acknowledgement = true;
    let mut driver =
        ScenarioDriver::new(1, Vec::new(), vec![expected], ScenarioLimits::default()).unwrap();
    let command = request(1, start());
    assert_eq!(
        driver.command(command.clone()),
        Err(ApplicationError::AcknowledgementUnknown {
            request: RequestId(1)
        })
    );
    assert!(list(&driver).tasks.is_empty());
    let acknowledgement = Acknowledgement {
        request: RequestId(1),
        task: TaskId(1),
    };
    assert_eq!(
        driver
            .query(query(Query::Request {
                request: RequestId(1)
            }))
            .unwrap(),
        QueryResponse::Request(RequestStatus::Accepted(acknowledgement))
    );
    assert_eq!(driver.command(command.clone()).unwrap(), acknowledgement);
    assert_eq!(driver.evidence().accepted, 1);
    assert!(driver.advance().unwrap());
    assert_eq!(
        driver
            .query(query(Query::Request {
                request: RequestId(1)
            }))
            .unwrap(),
        QueryResponse::Request(RequestStatus::Completed {
            acknowledgement,
            task_version: 1
        })
    );
    assert_eq!(driver.command(command).unwrap(), acknowledgement);
    assert!(!driver.advance().unwrap());
    assert!(driver.verify_complete().is_ok());
    assert_eq!(list(&driver).tasks.len(), 1);
}

#[test]
fn conflicting_request_ids_do_not_reuse_authority() {
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![step(start(), task(TaskState::Running, 1))],
        ScenarioLimits::default(),
    )
    .unwrap();
    driver.command(request(1, start())).unwrap();
    assert_eq!(
        driver.command(request(
            1,
            Command::Cancel {
                task: TaskId(1),
                expected_version: 1
            }
        )),
        Err(ApplicationError::RequestConflict)
    );
    assert_eq!(
        driver
            .query(query(Query::Request {
                request: RequestId(999)
            }))
            .unwrap(),
        QueryResponse::Request(RequestStatus::Unknown)
    );
}

#[test]
fn snapshot_subscription_handoff_includes_intervening_updates() {
    let mut driver = ScenarioDriver::new(
        7,
        Vec::new(),
        vec![step(start(), task(TaskState::Running, 1))],
        ScenarioLimits::default(),
    )
    .unwrap();
    let snapshot = list(&driver);
    driver.command(request(1, start())).unwrap();
    driver.advance().unwrap();
    let subscription = subscribe(&mut driver, snapshot.cursor);
    let batch = poll(&mut driver, subscription).unwrap();
    assert_eq!(batch.updates.len(), 1);
    assert_eq!(
        batch.updates[0].cursor,
        Cursor {
            epoch: 7,
            sequence: 1
        }
    );
    assert_eq!(batch.updates[0].request, Some(RequestId(1)));
    assert_eq!(batch.updates[0].task, list(&driver).tasks[0]);
    assert!(snapshot.tasks.is_empty());
    assert!(poll(&mut driver, subscription).unwrap().updates.is_empty());
}

#[test]
fn slow_subscribers_get_explicit_gaps_without_blocking_cancel() {
    let cancel = Command::Cancel {
        task: TaskId(1),
        expected_version: 1,
    };
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![
            step(start(), task(TaskState::Running, 1)),
            step(cancel.clone(), task(TaskState::Cancelled, 2)),
        ],
        ScenarioLimits {
            retained_updates: 1,
            ..ScenarioLimits::default()
        },
    )
    .unwrap();
    let initial = list(&driver).cursor;
    let slow = subscribe(&mut driver, initial);
    driver.command(request(1, start())).unwrap();
    driver.advance().unwrap();
    driver.command(request(2, cancel)).unwrap();
    driver.advance().unwrap();
    assert_eq!(
        poll(&mut driver, slow),
        Err(ApplicationError::ResyncRequired {
            current: Cursor {
                epoch: 1,
                sequence: 2
            }
        })
    );
    driver.detach(slow).unwrap();
    let snapshot = list(&driver);
    assert_eq!(snapshot.tasks[0].state, TaskState::Cancelled);
    let fresh = subscribe(&mut driver, snapshot.cursor);
    assert!(poll(&mut driver, fresh).unwrap().updates.is_empty());
    assert!(driver.verify_complete().is_ok());
}

#[test]
fn cursor_epochs_and_future_sequences_cannot_be_replayed() {
    let mut driver = demo_driver().unwrap();
    for after in [
        Cursor {
            epoch: 2,
            sequence: 0,
        },
        Cursor {
            epoch: 1,
            sequence: 1,
        },
    ] {
        assert!(matches!(
            driver.subscribe(SubscribeRequest {
                protocol_version: 1,
                after
            }),
            Err(ApplicationError::ResyncRequired { .. })
        ));
    }
}

#[test]
fn versioned_pages_require_resync_after_a_mutation() {
    let first = task(TaskState::Pending, 1);
    let mut second = first.clone();
    second.id = TaskId(2);
    let cancel = Command::Cancel {
        task: TaskId(1),
        expected_version: 1,
    };
    let mut driver = ScenarioDriver::new(
        1,
        vec![first, second],
        vec![step(cancel.clone(), task(TaskState::Cancelled, 2))],
        ScenarioLimits::default(),
    )
    .unwrap();
    let QueryResponse::Snapshot(page) = driver
        .query(query(Query::Tasks {
            after: None,
            limit: 1,
        }))
        .unwrap()
    else {
        panic!("expected page");
    };
    assert_eq!(page.tasks.len(), 1);
    let continuation = page.next_page.unwrap();
    let QueryResponse::Snapshot(next) = driver
        .query(query(Query::Tasks {
            after: Some(continuation),
            limit: 1,
        }))
        .unwrap()
    else {
        panic!("expected page");
    };
    assert_eq!(next.tasks[0].id, TaskId(2));
    assert!(next.next_page.is_none());
    driver.command(request(1, cancel)).unwrap();
    driver.advance().unwrap();
    assert!(matches!(
        driver.query(query(Query::Tasks {
            after: Some(continuation),
            limit: 1
        })),
        Err(ApplicationError::ResyncRequired { .. })
    ));
}

#[test]
fn stale_approval_and_unconfirmed_force_stop_are_rejected() {
    let mut waiting = task(TaskState::Waiting, 4);
    waiting.reason = Some(WaitReason::Permission);
    waiting.operation = Some(OperationSnapshot {
        id: OperationId(9),
        version: 3,
        state: OperationState::AwaitingApproval,
    });
    let approve = Command::Approve {
        task: TaskId(1),
        expected_version: 4,
        operation: OperationId(9),
        operation_version: 3,
    };
    let mut driver = ScenarioDriver::new(
        1,
        vec![waiting],
        vec![step(approve.clone(), task(TaskState::Running, 5))],
        ScenarioLimits::default(),
    )
    .unwrap();
    for (expected_version, operation_version, current) in [(3, 3, 4), (4, 2, 3)] {
        assert_eq!(
            driver.command(request(
                1,
                Command::Approve {
                    task: TaskId(1),
                    expected_version,
                    operation: OperationId(9),
                    operation_version
                }
            )),
            Err(ApplicationError::StaleVersion { current })
        );
    }
    assert_eq!(
        driver.command(request(
            2,
            Command::ForceStop {
                task: TaskId(1),
                expected_version: 4,
                confirmed: false
            }
        )),
        Err(ApplicationError::ConfirmationRequired)
    );
    assert_eq!(driver.evidence().accepted, 0);
    driver.command(request(1, approve)).unwrap();
    driver.advance().unwrap();
    assert!(driver.verify_complete().is_ok());
}

#[test]
fn structured_failure_is_not_completion_or_automatic_retry() {
    for error in [
        ApplicationError::PermissionDenied,
        ApplicationError::BudgetExhausted,
        ApplicationError::CapacityExhausted,
        ApplicationError::UnknownOutcome,
    ] {
        let mut driver = ScenarioDriver::new(
            1,
            Vec::new(),
            vec![ScenarioStep {
                expected: start(),
                result: Err(error.clone()),
                lose_acknowledgement: false,
            }],
            ScenarioLimits::default(),
        )
        .unwrap();
        let acknowledgement = driver.command(request(1, start())).unwrap();
        driver.advance().unwrap();
        assert_eq!(
            driver
                .query(query(Query::Request {
                    request: RequestId(1)
                }))
                .unwrap(),
            QueryResponse::Request(RequestStatus::Failed {
                acknowledgement,
                error
            })
        );
        assert!(list(&driver).tasks.is_empty());
        assert!(!driver.advance().unwrap());
        assert!(driver.verify_complete().is_ok());
    }
}

#[test]
fn detach_is_not_shutdown_and_shutdown_preserves_uncertainty() {
    let mut stopping = task(TaskState::Stopping, 1);
    stopping.reason = Some(WaitReason::TerminationUnconfirmed);
    stopping.attempt = Some(AttemptSnapshot {
        id: AttemptId(1),
        state: AttemptState::Interrupted,
    });
    stopping.operation = Some(OperationSnapshot {
        id: OperationId(1),
        version: 1,
        state: OperationState::Unknown,
    });
    let mut driver = ScenarioDriver::new(
        1,
        vec![stopping.clone()],
        Vec::new(),
        ScenarioLimits::default(),
    )
    .unwrap();
    let cursor = list(&driver).cursor;
    let subscription = subscribe(&mut driver, cursor);
    driver.detach(subscription).unwrap();
    assert_eq!(driver.host_state(), HostState::Running);
    assert_eq!(list(&driver).tasks, vec![stopping.clone()]);
    driver.quiesce();
    assert_eq!(
        driver.command(request(
            1,
            Command::Resume {
                task: TaskId(1),
                expected_version: 1
            }
        )),
        Err(ApplicationError::HostStopped)
    );
    let report = driver.shutdown();
    assert_eq!(report.unresolved_tasks, vec![TaskId(1)]);
    assert_eq!(list(&driver).tasks, vec![stopping]);
    assert_eq!(driver.advance(), Err(ApplicationError::HostStopped));
    assert_eq!(driver.shutdown(), report);
}

#[test]
fn bounds_fail_closed_and_request_records_are_not_evicted() {
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![step(start(), task(TaskState::Pending, 1))],
        ScenarioLimits {
            requests: 1,
            subscriptions: 1,
            ..ScenarioLimits::default()
        },
    )
    .unwrap();
    let cursor = list(&driver).cursor;
    let subscription = subscribe(&mut driver, cursor);
    assert_eq!(
        driver.subscribe(SubscribeRequest {
            protocol_version: 1,
            after: cursor
        }),
        Err(ApplicationError::CapacityExhausted)
    );
    driver.command(request(1, start())).unwrap();
    driver.advance().unwrap();
    assert_eq!(
        driver.command(request(
            2,
            Command::Cancel {
                task: TaskId(1),
                expected_version: 1
            }
        )),
        Err(ApplicationError::CapacityExhausted)
    );
    assert!(driver.command(request(1, start())).is_ok());
    driver.detach(subscription).unwrap();
    assert!(
        driver
            .query(query(Query::Tasks {
                after: None,
                limit: 0
            }))
            .is_err()
    );
    assert!(
        driver
            .query(query(Query::Tasks {
                after: None,
                limit: MAX_PAGE_SIZE + 1
            }))
            .is_err()
    );
}

#[test]
fn unexpected_and_missing_commands_fail_strict_scenarios() {
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![step(start(), task(TaskState::Running, 1))],
        ScenarioLimits::default(),
    )
    .unwrap();
    assert!(driver.verify_complete().is_err());
    let other = Command::Start {
        task: TaskId(2),
        session: SessionId(1),
        workspace: WorkspaceId(1),
        prompt: "unexpected".to_owned(),
    };
    assert_eq!(
        driver.command(request(2, other)),
        Err(ApplicationError::UnexpectedCommand)
    );
    driver.command(request(1, start())).unwrap();
    driver.advance().unwrap();
    assert_eq!(driver.evidence().unexpected, 1);
    assert!(driver.verify_complete().is_err());
}

#[test]
fn shutdown_cannot_be_reopened_and_reports_pending_acknowledgements() {
    let mut driver = ScenarioDriver::new(
        1,
        Vec::new(),
        vec![step(start(), task(TaskState::Running, 1))],
        ScenarioLimits::default(),
    )
    .unwrap();
    driver.command(request(1, start())).unwrap();
    assert_eq!(driver.shutdown().unresolved_requests, vec![RequestId(1)]);
    driver.quiesce();
    assert_eq!(driver.host_state(), HostState::Stopped);
    assert_eq!(driver.advance(), Err(ApplicationError::HostStopped));
    assert!(list(&driver).tasks.is_empty());
    assert!(matches!(
        driver
            .query(query(Query::Request {
                request: RequestId(1)
            }))
            .unwrap(),
        QueryResponse::Request(RequestStatus::Accepted(_))
    ));
}

#[test]
fn invalid_fixture_results_cannot_change_identity_or_reset_consumption() {
    let mut other = task(TaskState::Pending, 1);
    other.workspace = WorkspaceId(2);
    assert!(
        ScenarioDriver::new(
            1,
            Vec::new(),
            vec![step(start(), other)],
            ScenarioLimits::default()
        )
        .is_err()
    );
    let before = task(TaskState::Running, 3);
    let cancel = Command::Cancel {
        task: TaskId(1),
        expected_version: 3,
    };
    for result in [
        task(TaskState::Cancelled, 3),
        TaskSnapshot {
            active_milliseconds: 0,
            ..task(TaskState::Cancelled, 4)
        },
    ] {
        let mut driver = ScenarioDriver::new(
            1,
            vec![before.clone()],
            vec![step(cancel.clone(), result)],
            ScenarioLimits::default(),
        )
        .unwrap();
        assert_eq!(
            driver.command(request(1, cancel.clone())),
            Err(ApplicationError::InvalidRequest)
        );
        assert_eq!(list(&driver).tasks, vec![before.clone()]);
        assert_eq!(driver.evidence().accepted, 0);
    }
}

#[test]
fn protocol_payloads_round_trip_through_existing_serde_codec() {
    macro_rules! round_trip {
        ($type:ty, $value:expr) => {{
            let value: $type = $value;
            let text = toml_edit::ser::to_string(&value).unwrap();
            let restored: $type = toml_edit::de::from_str(&text).unwrap();
            assert_eq!(restored, value);
        }};
    }
    for command in [
        start(),
        Command::Resume {
            task: TaskId(1),
            expected_version: 1,
        },
        Command::Cancel {
            task: TaskId(1),
            expected_version: 1,
        },
        Command::ForceStop {
            task: TaskId(1),
            expected_version: 1,
            confirmed: true,
        },
        Command::Approve {
            task: TaskId(1),
            expected_version: 1,
            operation: OperationId(1),
            operation_version: 1,
        },
    ] {
        round_trip!(CommandRequest, request(1, command));
    }
    for state in [
        TaskState::Pending,
        TaskState::Ready,
        TaskState::Running,
        TaskState::Waiting,
        TaskState::Blocked,
        TaskState::Stopping,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ] {
        round_trip!(TaskSnapshot, task(state, 1));
    }
    let snapshot = list(&demo_driver().unwrap());
    round_trip!(Snapshot, snapshot.clone());
    round_trip!(QueryResponse, QueryResponse::Snapshot(snapshot.clone()));
    round_trip!(
        QueryRequest,
        query(Query::Tasks {
            after: Some(PageCursor {
                revision: snapshot.cursor,
                after: TaskId(1)
            }),
            limit: 1
        })
    );
    round_trip!(
        QueryRequest,
        query(Query::Request {
            request: RequestId(1)
        })
    );
    round_trip!(
        SubscribeRequest,
        SubscribeRequest {
            protocol_version: 1,
            after: snapshot.cursor
        }
    );
    round_trip!(
        PollRequest,
        PollRequest {
            protocol_version: 1,
            subscription: SubscriptionId(1),
            limit: 1
        }
    );
    round_trip!(
        UpdateBatch,
        UpdateBatch {
            protocol_version: 1,
            demo: true,
            through: snapshot.cursor,
            updates: vec![Update {
                cursor: snapshot.cursor,
                task: task(TaskState::Running, 1),
                request: Some(RequestId(1))
            }]
        }
    );
    let acknowledgement = Acknowledgement {
        request: RequestId(1),
        task: TaskId(1),
    };
    round_trip!(Acknowledgement, acknowledgement);
    round_trip!(RequestStatus, RequestStatus::Unknown);
    round_trip!(RequestStatus, RequestStatus::Accepted(acknowledgement));
    round_trip!(
        RequestStatus,
        RequestStatus::Completed {
            acknowledgement,
            task_version: 2
        }
    );
    for error in [
        ApplicationError::PermissionDenied,
        ApplicationError::BudgetExhausted,
        ApplicationError::UnknownOutcome,
        ApplicationError::AcknowledgementUnknown {
            request: RequestId(1),
        },
        ApplicationError::ResyncRequired {
            current: snapshot.cursor,
        },
    ] {
        round_trip!(
            RequestStatus,
            RequestStatus::Failed {
                acknowledgement,
                error
            }
        );
    }
    round_trip!(
        AttemptSnapshot,
        AttemptSnapshot {
            id: AttemptId(1),
            state: AttemptState::Interrupted
        }
    );
    round_trip!(
        OperationSnapshot,
        OperationSnapshot {
            id: OperationId(1),
            version: 3,
            state: OperationState::Unknown
        }
    );
}
