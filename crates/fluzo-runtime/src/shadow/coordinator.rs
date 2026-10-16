//! Shadow observer coordinator.
//!
//! Owns the enable/pause state, the per-task observation budget, the
//! one-per-decision-point rule and the bounded dispatch queue. The
//! agent path only ever calls [`ShadowCoordinator::submit`], which
//! is non-blocking: no shadow answer is ever awaited for
//! authorization or task progression (PRD 10.1). The coordinator
//! depends solely on the [`DecisionObserver`] trait, so replacing
//! the adapter needs no change here or in any consumer (A10).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, Semaphore, watch};

use super::{
    DecisionObserver, ObservationSink, ObservationStatus, ShadowObservationRecord, ShadowRequest,
};

/// Safeguard limits from `decision.*` settings (PRD 10.1).
#[derive(Clone, Debug)]
pub struct CoordinatorLimits {
    /// `decision.max_observations_per_task` (default 30).
    pub max_observations_per_task: u32,
    /// `decision.failure_pause_threshold` (default 3).
    pub failure_pause_threshold: u32,
    /// Bounded dispatch queue; overflow is dropped, never grown.
    pub queue_capacity: usize,
    /// Concurrent dispatched observations. Admission still gates
    /// the real capacity; this only bounds the coordinator's own
    /// fan-out.
    pub max_concurrent: usize,
}

impl Default for CoordinatorLimits {
    fn default() -> Self {
        Self {
            max_observations_per_task: 30,
            failure_pause_threshold: 3,
            queue_capacity: 16,
            max_concurrent: 1,
        }
    }
}

/// Why the observer is paused. A pause blocks new submissions until
/// an explicit [`ShadowCoordinator::resume`]; neither disabling nor
/// re-enabling lifts it (PRD 10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// Automatic: consecutive transport/protocol/timeout failures
    /// reached the threshold.
    ConsecutiveFailures { count: u32 },
    /// Explicit operator pause.
    Operator,
}

/// Immediate result of a non-blocking submit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission {
    Accepted,
    Disabled,
    Paused { reason: PauseReason },
    TaskBudgetExhausted,
    DuplicateDecisionPoint,
    QueueFull,
}

impl Submission {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Disabled => "disabled",
            Self::Paused { .. } => "paused",
            Self::TaskBudgetExhausted => "task_budget_exhausted",
            Self::DuplicateDecisionPoint => "duplicate_decision_point",
            Self::QueueFull => "queue_full",
        }
    }
}

/// Bounded effective-state snapshot for settings screens and
/// diagnostics: configured/effective enablement, queue depth and
/// the pause/skip reasons the UI must show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowStatusSnapshot {
    pub enabled: bool,
    pub paused: Option<PauseReason>,
    pub queued: usize,
    pub in_flight: usize,
    pub consecutive_failures: u32,
    pub last_rejection: Option<String>,
    pub per_task_observations: BTreeMap<String, u32>,
}

struct Queued {
    request: ShadowRequest,
    generation: u64,
}

struct State {
    enabled: bool,
    paused: Option<PauseReason>,
    per_task: BTreeMap<String, u32>,
    seen_points: BTreeSet<(String, String, u64, String)>,
    consecutive_failures: u32,
    queue: VecDeque<Queued>,
    in_flight: usize,
    generation: u64,
    cancel: watch::Sender<bool>,
    worker_started: bool,
    last_rejection: Option<String>,
}

impl State {
    fn new() -> Self {
        let (cancel, _rx) = watch::channel(false);
        Self {
            enabled: false,
            paused: None,
            per_task: BTreeMap::new(),
            seen_points: BTreeSet::new(),
            consecutive_failures: 0,
            queue: VecDeque::new(),
            in_flight: 0,
            generation: 0,
            cancel,
            worker_started: false,
            last_rejection: None,
        }
    }
}

/// The coordinator. Disabled by default; enabling is the only way
/// observations ever leave the process, and even then consent and
/// admission still apply downstream in the adapter.
pub struct ShadowCoordinator {
    limits: CoordinatorLimits,
    observer: Arc<dyn DecisionObserver>,
    sink: Arc<dyn ObservationSink>,
    state: Mutex<State>,
    notify: Arc<Notify>,
}

impl ShadowCoordinator {
    pub fn new(
        limits: CoordinatorLimits,
        observer: Arc<dyn DecisionObserver>,
        sink: Arc<dyn ObservationSink>,
    ) -> Self {
        Self {
            limits,
            observer,
            sink,
            state: Mutex::new(State::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Spawn the dispatch worker. Idempotent; safe to call before or
    /// after submits (queued items are picked up when it starts).
    pub fn start(self: &Arc<Self>) {
        let mut state = self.lock();
        if state.worker_started {
            return;
        }
        state.worker_started = true;
        drop(state);
        let coordinator = Arc::clone(self);
        tokio::spawn(async move { coordinator.worker().await });
    }

    /// Non-blocking submit. Returns immediately with the admission
    /// decision of this layer; the answer never gates anything.
    pub fn submit(&self, request: ShadowRequest) -> Submission {
        let mut state = self.lock();
        if !state.enabled {
            state.last_rejection = Some("disabled".to_owned());
            return Submission::Disabled;
        }
        if let Some(reason) = state.paused {
            state.last_rejection = Some("paused".to_owned());
            return Submission::Paused { reason };
        }
        let task = request.decision_point.task.clone();
        let used = state.per_task.get(&task).copied().unwrap_or(0);
        if used >= self.limits.max_observations_per_task {
            state.last_rejection = Some("task_budget_exhausted".to_owned());
            return Submission::TaskBudgetExhausted;
        }
        let key = request.decision_point.key();
        if state.seen_points.contains(&key) {
            state.last_rejection = Some("duplicate_decision_point".to_owned());
            return Submission::DuplicateDecisionPoint;
        }
        if state.queue.len() >= self.limits.queue_capacity {
            state.last_rejection = Some("queue_full".to_owned());
            return Submission::QueueFull;
        }
        state.per_task.insert(task, used + 1);
        state.seen_points.insert(key);
        let generation = state.generation;
        state.queue.push_back(Queued {
            request,
            generation,
        });
        drop(state);
        self.notify.notify_one();
        Submission::Accepted
    }

    /// Enable or disable the capability. Disabling stops new
    /// observations, records every queued one as cancelled
    /// (`queued_dropped`) and cancels in-flight ones best-effort;
    /// their admission slots remain occupied/uncertain in the
    /// controller regardless (PRD 10.1). A pause survives both
    /// disabling and re-enabling.
    pub fn set_enabled(&self, enabled: bool) {
        let dropped: VecDeque<Queued>;
        {
            let mut state = self.lock();
            state.enabled = enabled;
            if enabled {
                return;
            }
            dropped = std::mem::take(&mut state.queue);
            state.generation += 1;
            let _ = state.cancel.send(true);
            let (fresh, _rx) = watch::channel(false);
            state.cancel = fresh;
        }
        for queued in dropped {
            let mut record = queued.request.base_record();
            record.status = ObservationStatus::Cancelled {
                stage: "queued_dropped".to_owned(),
            };
            self.sink.submit(record);
        }
    }

    /// Explicit operator pause.
    pub fn pause(&self) {
        self.lock().paused = Some(PauseReason::Operator);
    }

    /// Explicit resume. Clears the failure counter so the same
    /// already-counted failures cannot instantly re-pause.
    pub fn resume(&self) {
        let mut state = self.lock();
        state.paused = None;
        state.consecutive_failures = 0;
    }

    pub fn status(&self) -> ShadowStatusSnapshot {
        let state = self.lock();
        ShadowStatusSnapshot {
            enabled: state.enabled,
            paused: state.paused,
            queued: state.queue.len(),
            in_flight: state.in_flight,
            consecutive_failures: state.consecutive_failures,
            last_rejection: state.last_rejection.clone(),
            per_task_observations: state.per_task.clone(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn worker(self: Arc<Self>) {
        let semaphore = Arc::new(Semaphore::new(self.limits.max_concurrent.max(1)));
        loop {
            let queued = {
                let mut state = self.lock();
                state.queue.pop_front()
            };
            let Some(queued) = queued else {
                // A notify permit stored by a submit that happened
                // before this await is consumed immediately, so
                // there is no lost-wakeup window; if the queue is
                // still empty on wake, the next iteration waits
                // again.
                self.notify.notified().await;
                continue;
            };
            let permit = Arc::clone(&semaphore).acquire_owned().await;
            let Ok(permit) = permit else {
                // Semaphore closed: record the drop explicitly.
                let mut record = queued.request.base_record();
                record.status = ObservationStatus::Cancelled {
                    stage: "worker_closed".to_owned(),
                };
                self.sink.submit(record);
                return;
            };
            // Re-check the generation after the permit wait: a
            // disable while queued here must still drop this one.
            let cancel_rx = {
                let mut state = self.lock();
                if queued.generation != state.generation {
                    drop(state);
                    let mut record = queued.request.base_record();
                    record.status = ObservationStatus::Cancelled {
                        stage: "queued_dropped".to_owned(),
                    };
                    self.sink.submit(record);
                    continue;
                }
                state.in_flight += 1;
                state.cancel.subscribe()
            };
            let coordinator = Arc::clone(&self);
            let observer = Arc::clone(&self.observer);
            let request = queued.request;
            tokio::spawn(async move {
                let record = observer.observe(request, cancel_rx).await;
                drop(permit);
                coordinator.on_completed(record);
            });
        }
    }

    fn on_completed(&self, record: ShadowObservationRecord) {
        {
            let mut state = self.lock();
            state.in_flight = state.in_flight.saturating_sub(1);
            if record.status.counts_as_failure() {
                state.consecutive_failures += 1;
                if state.consecutive_failures >= self.limits.failure_pause_threshold {
                    state.paused = Some(PauseReason::ConsecutiveFailures {
                        count: state.consecutive_failures,
                    });
                }
            } else if record.status == ObservationStatus::Completed {
                state.consecutive_failures = 0;
            }
        }
        self.sink.submit(record);
    }
}

/// In-memory collecting sink for tests and local inspection.
#[derive(Default)]
pub struct CollectingSink {
    records: Mutex<Vec<ShadowObservationRecord>>,
}

impl CollectingSink {
    pub fn records(&self) -> Vec<ShadowObservationRecord> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl ObservationSink for CollectingSink {
    fn submit(&self, record: ShadowObservationRecord) {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(record);
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
