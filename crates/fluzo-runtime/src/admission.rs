//! RUN-05 per-process inference admission controller.
//!
//! PRD v0.2 section 43.3 with Architecture A04. This is the live
//! counterpart to the pure rules in `fluzo_core::admission`: one
//! controller per runtime process, holding per-pool and per-model
//! capacity-identity counters, a FIFO foreground queue, and uncertain
//! slot bookkeeping.
//!
//! Deliberate properties:
//!
//! - **Never blocks.** Every call returns a value immediately. Waiting
//!   is represented by a queued ticket with a deadline, not by parking
//!   a thread or a task, so queueing can never delay input handling or
//!   cancellation.
//! - **Injectable time.** Every method takes `now_ms` from the caller;
//!   the module reads no clock. Tests drive time deterministically and
//!   production passes a monotonic millisecond clock.
//! - **No cross-process coordination.** Two controllers with identical
//!   pool names keep independent counters by construction (A04). There
//!   is no shared journal, lock or peer lookup to coordinate through.
//! - **No bypass.** There is no parameter that skips admission. YOLO
//!   removes prompts (RUN-02), not capacity limits, and this module
//!   has no knowledge of prompts at all.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;

pub use fluzo_core::admission::PoolLimits;
use fluzo_core::admission::{
    AdmissionDecision, BudgetConsumption, BudgetExhausted, PoolSnapshot, RequestClass,
    ShadowSkipReason, TaskBudgets, check_budgets, decide_admission, reserve_inference_tokens,
};

/// Named capacity pool grouping this process's requests to a shared
/// inference server or resource group. Names are process-local: the
/// same name in another process means nothing here.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PoolId(pub String);

/// Explicit capacity identity of a backend model. Aliases of the same
/// backend model MUST be registered under one `ModelId` so they share
/// one local allowance instead of multiplying it.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ModelId(pub String);

/// Handle for a queued waiter.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Ticket(pub u64);

/// Handle for an admitted (dispatched or in-flight) slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SlotId(pub u64);

/// Everything admission needs about one request.
#[derive(Clone, Debug)]
pub struct AdmissionRequest {
    pub class: RequestClass,
    pub pool: PoolId,
    pub model: ModelId,
    /// Conservative input estimate for the token reservation. The
    /// caller owns the estimation policy; this layer never assumes an
    /// absent estimate is zero, so the caller must pass a documented
    /// conservative value.
    pub input_estimate_tokens: u64,
    /// Maximum response output the request may produce.
    pub max_response_output_tokens: u64,
    /// Remaining active-time budget for the owning task, in whole
    /// seconds. Bounds the queue deadline. A task with no owning budget
    /// passes a generous-but-finite value; there is no infinite wait.
    pub remaining_task_seconds: u64,
    /// Task budgets for the pre-admission check. `None` skips the
    /// budget gate (for callers whose work has no task budget, such as
    /// setup probes) and is distinct from a budget of zero.
    pub budgets: Option<TaskBudgets>,
    /// Consumption paired with `budgets`. Must be `Some` together with
    /// `budgets`; a mismatch is rejected as `BudgetsIncomplete`.
    pub consumed: Option<BudgetConsumption>,
}

/// Immediate outcome of a `try_admit` call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    /// Capacity reserved for this request. Dispatch and later release
    /// the slot. The slot is occupied from this moment, before any
    /// socket opens; a streaming request holds it until completion,
    /// not until first token.
    Admitted(SlotId),
    /// No capacity now. The request waits in the FIFO foreground queue
    /// until `deadline_ms`. `position` is 1-based among foreground
    /// waiters.
    Queued {
        ticket: Ticket,
        position: u32,
        deadline_ms: u64,
    },
    /// Not admitted, with the machine-readable reason.
    Denied(AdmissionDenied),
}

/// Typed denial. Each variant is a distinct visible outcome; none of
/// them carries an implicit retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionDenied {
    /// The pool queue is full (`capacity_exhausted`).
    CapacityExhausted,
    /// A queued wait reached its deadline (`capacity_wait_timeout`).
    CapacityWaitTimeout,
    /// A task budget was reached before admission.
    BudgetExhausted(BudgetExhausted),
    /// A shadow observation was skipped rather than made to compete.
    ShadowSkipped { reason: ShadowSkipReason },
    /// No pool registered under this name.
    UnknownPool,
    /// The pool exists but this model capacity identity was not
    /// registered into it.
    UnknownModel,
    /// `budgets` supplied without `consumed` (or the reverse).
    BudgetsIncomplete,
}

impl AdmissionDenied {
    /// Stable machine-readable code for headless results and metrics.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CapacityExhausted => "capacity_exhausted",
            Self::CapacityWaitTimeout => "capacity_wait_timeout",
            Self::BudgetExhausted(_) => "budget_exhausted",
            Self::ShadowSkipped { .. } => "shadow_skipped",
            Self::UnknownPool => "unknown_pool",
            Self::UnknownModel => "unknown_model",
            Self::BudgetsIncomplete => "budgets_incomplete",
        }
    }
}

/// How a dispatched request finished, as reported by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseKind {
    /// Response fully consumed (success or a terminal typed failure).
    /// The slot is released.
    Completed,
    /// Timed out or disconnected after dispatch, with remote completion
    /// unconfirmed. The slot stays occupied as uncertain until explicit
    /// reconcile evidence. Dropping the request is not proof the
    /// remote work stopped.
    Uncertain,
}

/// Evidence that resolves an uncertain slot. Only an explicit
/// reconciliation frees it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconcileEvidence {
    /// Confirmed the request finished remotely.
    Completed,
    /// Confirmed the request was cancelled remotely.
    Cancelled,
}

/// Bounded view of one pool for UI and metrics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolView {
    pub limits: PoolLimits,
    pub in_flight: u32,
    pub uncertain: u32,
    pub queued_foreground: u32,
    /// Milliseconds the longest-waiting foreground request has been
    /// queued as of the view time. Queue duration is reported
    /// separately from inference time (PRD 43.3).
    pub oldest_queue_wait_ms: u64,
    /// Per-model occupancy including uncertain slots.
    pub models: BTreeMap<String, ModelOccupancy>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct ModelOccupancy {
    pub limit: u32,
    pub in_flight: u32,
    pub uncertain: u32,
}

struct Slot {
    ticket: Ticket,
    class: RequestClass,
    model: ModelId,
}

struct Waiter {
    ticket: Ticket,
    class: RequestClass,
    model: ModelId,
    queued_at_ms: u64,
    deadline_ms: u64,
}

struct PoolEntry {
    limits: PoolLimits,
    queue_timeout_ms: u64,
    model_limits: BTreeMap<ModelId, u32>,
    in_flight: BTreeMap<SlotId, Slot>,
    uncertain: BTreeMap<SlotId, Slot>,
    queue: VecDeque<Waiter>,
}

impl PoolEntry {
    fn snapshot(&self) -> PoolSnapshot {
        PoolSnapshot {
            in_flight: self.in_flight.len() as u32,
            uncertain: self.uncertain.len() as u32,
            queued_foreground: self
                .queue
                .iter()
                .filter(|waiter| waiter.class.is_foreground())
                .count() as u32,
            queued_shadow: self
                .queue
                .iter()
                .filter(|waiter| !waiter.class.is_foreground())
                .count() as u32,
        }
    }

    fn model_occupied(&self, model: &ModelId) -> u32 {
        self.in_flight
            .values()
            .chain(self.uncertain.values())
            .filter(|slot| &slot.model == model)
            .count() as u32
    }
}

/// One controller per runtime process. All state is behind one mutex;
/// every operation is O(queue length) at worst and never blocks on I/O.
pub struct AdmissionController {
    state: Mutex<ControllerState>,
}

struct ControllerState {
    pools: BTreeMap<PoolId, PoolEntry>,
    next_id: u64,
}

/// Registration error for a pool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterError {
    Limits(PoolLimitsValidationError),
    DuplicatePool,
    ZeroModelLimit,
}

/// Re-export-free wrapper keeps the core limits error name stable here.
pub type PoolLimitsValidationError = fluzo_core::admission::LimitsError;

/// Queue timeout sanity ceiling in milliseconds (10 minutes). The PRD
/// default is 30 seconds.
pub const MAX_QUEUE_TIMEOUT_MS: u64 = 600_000;

impl AdmissionController {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ControllerState {
                pools: BTreeMap::new(),
                next_id: 1,
            }),
        }
    }

    /// Register a pool with its limits, queue timeout and the model
    /// capacity identities that may draw from it.
    pub fn register_pool(
        &self,
        pool: PoolId,
        limits: PoolLimits,
        queue_timeout_ms: u64,
        model_limits: BTreeMap<ModelId, u32>,
    ) -> Result<(), RegisterError> {
        if queue_timeout_ms == 0 || queue_timeout_ms > MAX_QUEUE_TIMEOUT_MS {
            return Err(RegisterError::Limits(
                fluzo_core::admission::LimitsError::QueueCapacityTooLarge,
            ));
        }
        if model_limits.values().any(|limit| *limit == 0) {
            return Err(RegisterError::ZeroModelLimit);
        }
        let mut state = self.lock();
        if state.pools.contains_key(&pool) {
            return Err(RegisterError::DuplicatePool);
        }
        state.pools.insert(
            pool,
            PoolEntry {
                limits,
                queue_timeout_ms,
                model_limits,
                in_flight: BTreeMap::new(),
                uncertain: BTreeMap::new(),
                queue: VecDeque::new(),
            },
        );
        Ok(())
    }

    /// Replace a pool's limits (drain semantics). Already-admitted
    /// requests keep running; new admission is blocked while occupancy
    /// is at or above the new ceiling, and the pool drains as existing
    /// work completes (PRD 43.3).
    pub fn apply_limits(&self, pool: &PoolId, limits: PoolLimits) -> Result<(), AdmissionDenied> {
        let mut state = self.lock();
        let entry = state
            .pools
            .get_mut(pool)
            .ok_or(AdmissionDenied::UnknownPool)?;
        entry.limits = limits;
        Ok(())
    }

    /// Decide one request. Never blocks; see the module contract.
    pub fn try_admit(&self, request: &AdmissionRequest, now_ms: u64) -> Admission {
        let mut state = self.lock();
        // Reserve the id before borrowing the pool entry so the borrow
        // checker never sees `state` used through two paths at once.
        let fresh_id = state.next_id;
        state.next_id += 1;
        let Some(entry) = state.pools.get_mut(&request.pool) else {
            return Admission::Denied(AdmissionDenied::UnknownPool);
        };
        let Some(&model_limit) = entry.model_limits.get(&request.model) else {
            return Admission::Denied(AdmissionDenied::UnknownModel);
        };
        match (&request.budgets, &request.consumed) {
            (None, None) => {}
            (Some(budgets), Some(consumed)) => {
                if let Err(exhausted) = check_budgets(budgets, consumed) {
                    return Admission::Denied(AdmissionDenied::BudgetExhausted(exhausted));
                }
                if let Err(exhausted) = reserve_inference_tokens(
                    budgets,
                    consumed,
                    request.input_estimate_tokens,
                    request.max_response_output_tokens,
                ) {
                    return Admission::Denied(AdmissionDenied::BudgetExhausted(exhausted));
                }
            }
            _ => return Admission::Denied(AdmissionDenied::BudgetsIncomplete),
        }

        let model_at_limit = entry.model_occupied(&request.model) >= model_limit;
        let snapshot = entry.snapshot();
        match decide_admission(request.class, model_at_limit, &snapshot, &entry.limits) {
            AdmissionDecision::Admit => {
                let slot = SlotId(fresh_id);
                entry.in_flight.insert(
                    slot,
                    Slot {
                        ticket: Ticket(slot.0),
                        class: request.class,
                        model: request.model.clone(),
                    },
                );
                Admission::Admitted(slot)
            }
            AdmissionDecision::Queue => {
                let deadline_ms = admission_deadline(
                    now_ms,
                    entry.queue_timeout_ms,
                    request.remaining_task_seconds,
                );
                if deadline_ms <= now_ms {
                    // No time remains to wait even momentarily; the
                    // wait would already have expired.
                    return Admission::Denied(AdmissionDenied::CapacityWaitTimeout);
                }
                let ticket = Ticket(fresh_id);
                let position = entry
                    .queue
                    .iter()
                    .filter(|waiter| waiter.class.is_foreground())
                    .count() as u32
                    + 1;
                entry.queue.push_back(Waiter {
                    ticket,
                    class: request.class,
                    model: request.model.clone(),
                    queued_at_ms: now_ms,
                    deadline_ms,
                });
                Admission::Queued {
                    ticket,
                    position,
                    deadline_ms,
                }
            }
            AdmissionDecision::QueueFull => Admission::Denied(AdmissionDenied::CapacityExhausted),
            AdmissionDecision::ShadowSkipped(reason) => {
                Admission::Denied(AdmissionDenied::ShadowSkipped { reason })
            }
        }
    }

    /// Hand freed capacity to the longest-waiting foreground request
    /// (FIFO fairness). Returns the ticket that may now dispatch with
    /// its reserved slot, or `None` if no waiter fits. Expired
    /// waiters are dropped rather than admitted; `expire` reports
    /// them. Strict FIFO means a head waiter blocked by its own model
    /// ceiling holds the queue: the pool never reorders waiting work.
    pub fn admit_next(&self, pool: &PoolId, now_ms: u64) -> Option<(Ticket, SlotId)> {
        let mut state = self.lock();
        let fresh_id = state.next_id;
        state.next_id += 1;
        let entry = state.pools.get_mut(pool)?;
        loop {
            let head = entry.queue.front()?;
            if head.deadline_ms <= now_ms {
                let waiter = entry.queue.pop_front().expect("front checked");
                // Timed-out head is reported by expire(); drop it here
                // so fairness never admits an expired request.
                let _ = waiter;
                continue;
            }
            let model_limit = entry.model_limits.get(&head.model).copied().unwrap_or(0);
            let model_at_limit = entry.model_occupied(&head.model) >= model_limit;
            let snapshot = entry.snapshot();
            // The head is at the front of the queue, so the fairness
            // branch of decide_admission would re-queue it. Evaluate
            // raw capacity for the head only.
            let capacity_free = !model_at_limit && snapshot.occupied() < entry.limits.max_in_flight;
            if !capacity_free {
                return None;
            }
            let waiter = entry.queue.pop_front().expect("front checked");
            let slot = SlotId(fresh_id);
            entry.in_flight.insert(
                slot,
                Slot {
                    ticket: waiter.ticket,
                    class: waiter.class,
                    model: waiter.model,
                },
            );
            return Some((waiter.ticket, slot));
        }
    }

    /// Time out every queued waiter whose deadline has passed.
    /// Returns the timed-out tickets; none of them was dispatched.
    pub fn expire(&self, now_ms: u64) -> Vec<Ticket> {
        let mut state = self.lock();
        let mut expired = Vec::new();
        for entry in state.pools.values_mut() {
            let mut kept = VecDeque::new();
            while let Some(waiter) = entry.queue.pop_front() {
                if waiter.deadline_ms <= now_ms {
                    expired.push(waiter.ticket);
                } else {
                    kept.push_back(waiter);
                }
            }
            entry.queue = kept;
        }
        expired.sort_unstable();
        expired
    }

    /// Remove a queued request without dispatch. Returns `false` if
    /// the ticket is not queued (already admitted, expired or
    /// unknown).
    pub fn cancel_queued(&self, ticket: Ticket) -> bool {
        let mut state = self.lock();
        for entry in state.pools.values_mut() {
            if let Some(index) = entry
                .queue
                .iter()
                .position(|waiter| waiter.ticket == ticket)
            {
                entry.queue.remove(index);
                return true;
            }
        }
        false
    }

    /// Report how a dispatched request ended.
    ///
    /// `Completed` frees the slot. `Uncertain` moves it to the
    /// uncertain set where it keeps consuming pool and model capacity
    /// until [`AdmissionController::reconcile`]. Freeing capacity on
    /// a dropped future would let an unresolved request be repeated,
    /// which PRD 43.3 forbids.
    pub fn release(&self, slot: SlotId, kind: ReleaseKind) -> bool {
        let mut state = self.lock();
        for entry in state.pools.values_mut() {
            if let Some(occupied) = entry.in_flight.remove(&slot) {
                match kind {
                    ReleaseKind::Completed => return true,
                    ReleaseKind::Uncertain => {
                        entry.uncertain.insert(slot, occupied);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Resolve an uncertain slot with explicit evidence. Frees the
    /// slot and returns `true` only for a previously uncertain slot.
    /// No automatic path calls this: uncertainty ends on confirmed
    /// completion/cancellation evidence or an operator-acknowledged
    /// reset, never on a timer.
    pub fn reconcile(&self, slot: SlotId, evidence: ReconcileEvidence) -> bool {
        let mut state = self.lock();
        for entry in state.pools.values_mut() {
            if entry.uncertain.remove(&slot).is_some() {
                let _ = evidence;
                return true;
            }
        }
        false
    }

    /// Bounded snapshot of one pool.
    pub fn view(&self, pool: &PoolId, now_ms: u64) -> Option<PoolView> {
        let state = self.lock();
        let entry = state.pools.get(pool)?;
        let mut models = BTreeMap::new();
        for (model, limit) in &entry.model_limits {
            let occupancy = models
                .entry(model.0.clone())
                .or_insert_with(ModelOccupancy::default);
            occupancy.limit = *limit;
            occupancy.in_flight = entry
                .in_flight
                .values()
                .filter(|slot| &slot.model == model)
                .count() as u32;
            occupancy.uncertain = entry
                .uncertain
                .values()
                .filter(|slot| &slot.model == model)
                .count() as u32;
        }
        let oldest_queue_wait_ms = entry
            .queue
            .front()
            .map(|waiter| now_ms.saturating_sub(waiter.queued_at_ms))
            .unwrap_or(0);
        Some(PoolView {
            limits: entry.limits,
            in_flight: entry.in_flight.len() as u32,
            uncertain: entry.uncertain.len() as u32,
            queued_foreground: entry
                .queue
                .iter()
                .filter(|waiter| waiter.class.is_foreground())
                .count() as u32,
            oldest_queue_wait_ms,
            models,
        })
    }

    /// Tickets currently admitted for a pool, for caller bookkeeping.
    pub fn admitted_slots(&self, pool: &PoolId) -> Option<BTreeSet<SlotId>> {
        let state = self.lock();
        let entry = state.pools.get(pool)?;
        Some(entry.in_flight.keys().copied().collect())
    }

    /// Correlate an admitted slot back to its request ticket and
    /// class, for callers that must report or reconcile by identity.
    pub fn slot_info(&self, slot: SlotId) -> Option<(Ticket, RequestClass)> {
        let state = self.lock();
        for entry in state.pools.values() {
            if let Some(occupied) = entry.in_flight.get(&slot) {
                return Some((occupied.ticket, occupied.class));
            }
        }
        None
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ControllerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for AdmissionController {
    fn default() -> Self {
        Self::new()
    }
}

/// Queue deadline: the pool timeout bounded by the task's remaining
/// active time, both in milliseconds.
fn admission_deadline(now_ms: u64, queue_timeout_ms: u64, remaining_task_seconds: u64) -> u64 {
    let task_budget_ms = remaining_task_seconds.saturating_mul(1000);
    now_ms.saturating_add(queue_timeout_ms.min(task_budget_ms))
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod admission_tests;
