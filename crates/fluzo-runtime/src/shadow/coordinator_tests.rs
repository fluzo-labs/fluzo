use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, watch};

use super::*;
use crate::shadow::{DecisionPoint, ObservationStatus, ShadowRequest};

struct StubObserver {
    outcome: ObservationStatus,
    scripted: Mutex<VecDeque<ObservationStatus>>,
    gate: Mutex<Option<oneshot::Receiver<()>>>,
    calls: AtomicUsize,
}

impl StubObserver {
    fn new(outcome: ObservationStatus) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            scripted: Mutex::new(VecDeque::new()),
            gate: Mutex::new(None),
            calls: AtomicUsize::new(0),
        })
    }

    fn scripted(outcomes: Vec<ObservationStatus>) -> Arc<Self> {
        let fallback = outcomes
            .last()
            .cloned()
            .unwrap_or(ObservationStatus::Completed);
        Arc::new(Self {
            outcome: fallback,
            scripted: Mutex::new(outcomes.into()),
            gate: Mutex::new(None),
            calls: AtomicUsize::new(0),
        })
    }

    fn next_outcome(&self) -> ObservationStatus {
        self.scripted
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.outcome.clone())
    }

    fn with_gate(gate: oneshot::Receiver<()>) -> Arc<Self> {
        Arc::new(Self {
            outcome: ObservationStatus::Completed,
            scripted: Mutex::new(VecDeque::new()),
            gate: Mutex::new(Some(gate)),
            calls: AtomicUsize::new(0),
        })
    }
}

impl DecisionObserver for StubObserver {
    fn observe(
        self: Arc<Self>,
        request: ShadowRequest,
        mut cancel: watch::Receiver<bool>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ShadowObservationRecord> + Send + 'static>,
    > {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let mut record = request.base_record();
            let gate = self.gate.lock().unwrap().take();
            if let Some(gate) = gate {
                tokio::select! {
                    biased;
                    _ = cancel.wait_for(|cancelled| *cancelled) => {
                        record.status = ObservationStatus::Cancelled {
                            stage: "after_dispatch".to_owned(),
                        };
                        return record;
                    }
                    _ = gate => {}
                }
            }
            record.status = self.next_outcome();
            record
        })
    }
}

fn request(task: &str, stage: &str, version: u64) -> ShadowRequest {
    ShadowRequest {
        observation_id: format!("obs-{task}-{stage}-{version}"),
        decision_point: DecisionPoint {
            task: task.to_owned(),
            stage: stage.to_owned(),
            state_version: version,
            state_fingerprint: format!("fp-{version}"),
        },
        question: crate::shadow::wire::ShadowQuestion::Noul {
            prompt: "proceed?".to_owned(),
        },
        model: "stub".to_owned(),
        input_estimate_tokens: 10,
        remaining_task_seconds: 60,
        state_size_bytes: 10,
        language: None,
        truncation: None,
    }
}

fn coordinator_with(
    limits: CoordinatorLimits,
    observer: Arc<dyn DecisionObserver>,
) -> (Arc<ShadowCoordinator>, Arc<CollectingSink>) {
    let sink = Arc::new(CollectingSink::default());
    let dyn_sink: Arc<dyn ObservationSink> = sink.clone();
    let coordinator = Arc::new(ShadowCoordinator::new(limits, observer, dyn_sink));
    (coordinator, sink)
}

async fn wait_records(sink: &Arc<CollectingSink>, count: usize) -> Vec<ShadowObservationRecord> {
    let started = Instant::now();
    loop {
        let records = sink.records();
        if records.len() >= count {
            return records;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timed out with {} records: {:?}",
            records.len(),
            records
                .iter()
                .map(|record| record.status.code())
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn disabled_by_default_rejects_without_records() {
    let observer = StubObserver::new(ObservationStatus::Completed);
    let (coordinator, sink) = coordinator_with(CoordinatorLimits::default(), observer.clone());
    coordinator.start();
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Disabled
    );
    assert_eq!(observer.calls.load(Ordering::SeqCst), 0);
    assert!(sink.records().is_empty());
}

#[tokio::test]
async fn enabled_flow_records_completed_observation() {
    let observer = StubObserver::new(ObservationStatus::Completed);
    let (coordinator, sink) = coordinator_with(CoordinatorLimits::default(), observer.clone());
    coordinator.start();
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Accepted
    );
    let records = wait_records(&sink, 1).await;
    assert_eq!(records[0].status, ObservationStatus::Completed);
    assert_eq!(observer.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn substitute_observer_needs_no_coordinator_change() {
    // A10: the coordinator only knows the trait. This substitute is
    // a pure in-memory adapter with a non-success outcome; the whole
    // flow still works and records the substitute's status.
    let observer = StubObserver::new(ObservationStatus::Rejected {
        reason: crate::shadow::wire::AnswerRejection::UnknownAnswerType,
    });
    let (coordinator, sink) = coordinator_with(CoordinatorLimits::default(), observer.clone());
    coordinator.start();
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Accepted
    );
    let records = wait_records(&sink, 1).await;
    assert_eq!(
        records[0].status,
        ObservationStatus::Rejected {
            reason: crate::shadow::wire::AnswerRejection::UnknownAnswerType
        }
    );
}

#[tokio::test]
async fn per_task_budget_blocks_extra_observations() {
    let limits = CoordinatorLimits {
        max_observations_per_task: 2,
        ..CoordinatorLimits::default()
    };
    let observer = StubObserver::new(ObservationStatus::Completed);
    let (coordinator, _sink) = coordinator_with(limits, observer);
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Accepted
    );
    assert_eq!(
        coordinator.submit(request("t", "s2", 2)),
        Submission::Accepted
    );
    assert_eq!(
        coordinator.submit(request("t", "s3", 3)),
        Submission::TaskBudgetExhausted
    );
    // A different task keeps its full budget.
    assert_eq!(
        coordinator.submit(request("u", "s1", 1)),
        Submission::Accepted
    );
}

#[tokio::test]
async fn one_observation_per_decision_point() {
    let observer = StubObserver::new(ObservationStatus::Completed);
    let (coordinator, _sink) = coordinator_with(CoordinatorLimits::default(), observer);
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Accepted
    );
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::DuplicateDecisionPoint
    );
}

#[tokio::test]
async fn queue_is_bounded() {
    let limits = CoordinatorLimits {
        queue_capacity: 1,
        ..CoordinatorLimits::default()
    };
    let observer = StubObserver::new(ObservationStatus::Completed);
    // Worker intentionally not started so the queue cannot drain.
    let (coordinator, _sink) = coordinator_with(limits, observer);
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s1", 1)),
        Submission::Accepted
    );
    assert_eq!(
        coordinator.submit(request("t", "s2", 2)),
        Submission::QueueFull
    );
}

#[tokio::test]
async fn pauses_after_consecutive_failures() {
    let limits = CoordinatorLimits {
        failure_pause_threshold: 2,
        ..CoordinatorLimits::default()
    };
    let observer = StubObserver::new(ObservationStatus::Failed {
        code: "transport".to_owned(),
    });
    let (coordinator, sink) = coordinator_with(limits, observer);
    coordinator.start();
    coordinator.set_enabled(true);
    coordinator.submit(request("t", "s1", 1));
    coordinator.submit(request("t", "s2", 2));
    wait_records(&sink, 2).await;
    let status = coordinator.status();
    assert_eq!(
        status.paused,
        Some(PauseReason::ConsecutiveFailures { count: 2 })
    );
    assert_eq!(
        coordinator.submit(request("t", "s3", 3)),
        Submission::Paused {
            reason: PauseReason::ConsecutiveFailures { count: 2 }
        }
    );
}

#[tokio::test]
async fn success_resets_the_failure_streak() {
    let limits = CoordinatorLimits {
        failure_pause_threshold: 2,
        // Serialize processing so the scripted outcome order is the
        // observed streak order.
        max_concurrent: 1,
        ..CoordinatorLimits::default()
    };
    // fail, success, fail: never two in a row, so no pause.
    let observer = StubObserver::scripted(vec![
        ObservationStatus::Failed {
            code: "transport".to_owned(),
        },
        ObservationStatus::Completed,
        ObservationStatus::Failed {
            code: "transport".to_owned(),
        },
    ]);
    let (coordinator, sink) = coordinator_with(limits, observer);
    coordinator.start();
    coordinator.set_enabled(true);
    coordinator.submit(request("t", "s1", 1));
    coordinator.submit(request("t", "s2", 2));
    coordinator.submit(request("t", "s3", 3));
    wait_records(&sink, 3).await;
    assert_eq!(coordinator.status().paused, None);
    assert_eq!(coordinator.status().consecutive_failures, 1);
}

#[tokio::test]
async fn resume_clears_pause_and_counter() {
    let limits = CoordinatorLimits {
        failure_pause_threshold: 1,
        ..CoordinatorLimits::default()
    };
    let observer = StubObserver::new(ObservationStatus::Timeout {
        stage: "request".to_owned(),
    });
    let (coordinator, sink) = coordinator_with(limits, observer);
    coordinator.start();
    coordinator.set_enabled(true);
    coordinator.submit(request("t", "s1", 1));
    wait_records(&sink, 1).await;
    assert!(coordinator.status().paused.is_some());
    assert_eq!(
        coordinator.submit(request("t", "s2", 2)),
        Submission::Paused {
            reason: PauseReason::ConsecutiveFailures { count: 1 }
        }
    );
    coordinator.resume();
    assert_eq!(coordinator.status().paused, None);
    assert_eq!(
        coordinator.submit(request("t", "s2", 2)),
        Submission::Accepted
    );
}

#[tokio::test]
async fn re_enabling_does_not_lift_a_pause() {
    let limits = CoordinatorLimits {
        failure_pause_threshold: 1,
        ..CoordinatorLimits::default()
    };
    let observer = StubObserver::new(ObservationStatus::Failed {
        code: "transport".to_owned(),
    });
    let (coordinator, sink) = coordinator_with(limits, observer);
    coordinator.start();
    coordinator.set_enabled(true);
    coordinator.submit(request("t", "s1", 1));
    wait_records(&sink, 1).await;
    coordinator.set_enabled(false);
    coordinator.set_enabled(true);
    assert_eq!(
        coordinator.submit(request("t", "s2", 2)),
        Submission::Paused {
            reason: PauseReason::ConsecutiveFailures { count: 1 }
        }
    );
}

#[tokio::test]
async fn disable_drops_queued_and_cancels_in_flight() {
    let limits = CoordinatorLimits {
        max_concurrent: 1,
        queue_capacity: 8,
        ..CoordinatorLimits::default()
    };
    let (gate_tx, gate_rx) = oneshot::channel();
    let observer = StubObserver::with_gate(gate_rx);
    let (coordinator, sink) = coordinator_with(limits, observer.clone());
    coordinator.start();
    coordinator.set_enabled(true);
    // A is dispatched and blocks in the observer; B and C queue.
    coordinator.submit(request("t", "a", 1));
    // Give the worker one dispatch cycle so A is in flight.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        coordinator.submit(request("t", "b", 2)),
        Submission::Accepted
    );
    assert_eq!(
        coordinator.submit(request("t", "c", 3)),
        Submission::Accepted
    );
    coordinator.set_enabled(false);
    // The in-flight stub sees the cancellation flag and reports it.
    drop(gate_tx);
    let records = wait_records(&sink, 3).await;
    let dropped = records
        .iter()
        .filter(|record| record.status.code() == "cancelled_queued_dropped")
        .count();
    let in_flight_cancelled = records
        .iter()
        .filter(|record| record.status.code() == "cancelled_after_dispatch")
        .count();
    assert_eq!(
        dropped, 2,
        "queued observations must be dropped: {records:?}"
    );
    assert_eq!(
        in_flight_cancelled, 1,
        "the in-flight observation must be cancelled best-effort: {records:?}"
    );
    assert_eq!(coordinator.status().queued, 0);
}

#[tokio::test]
async fn submit_never_blocks_on_a_slow_observer() {
    let (_gate_tx, gate_rx) = oneshot::channel::<()>();
    let observer = StubObserver::with_gate(gate_rx);
    let (coordinator, _sink) = coordinator_with(CoordinatorLimits::default(), observer);
    coordinator.start();
    coordinator.set_enabled(true);
    coordinator.submit(request("t", "a", 1));
    let started = Instant::now();
    let second = coordinator.submit(request("t", "b", 2));
    let elapsed = started.elapsed();
    assert_eq!(second, Submission::Accepted);
    assert!(
        elapsed < Duration::from_millis(100),
        "submit blocked for {elapsed:?}"
    );
    let status = coordinator.status();
    assert!(status.in_flight >= 1 || status.queued >= 1);
}
