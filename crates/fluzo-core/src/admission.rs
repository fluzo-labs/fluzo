//! RUN-05 pure admission and budget rules.
//!
//! PRD v0.2 sections 43.1, 43.2 and 43.3 with Architecture A04. These
//! are the decision rules only: the live per-process controller lives in
//! `fluzo-runtime::admission` and calls into here. No I/O, no clock
//! reads and no shared mutable state; every rule is a function of the
//! values passed in, so the same rules that gate real requests are the
//! ones under test.
//!
//! Scope discipline from A04: these limits are per-process ceilings.
//! Nothing here coordinates with another Fluzo process, owns a model
//! server, or manages provider rate limits. A rejection is a typed
//! outcome for the caller to surface, never a trigger for automatic
//! recovery.

use crate::settings::HarnessSettings;

/// Which request path is asking for capacity. The class decides
/// foreground/shadow treatment, not priority within the foreground
/// queue: all foreground requests are FIFO-equal (PRD 43.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RequestClass {
    /// A logical agent-model interaction.
    AgentTurn,
    /// Context compaction needed to continue a task. Foreground work,
    /// not an exemption from the limits.
    Compaction,
    /// An authorized setup probe (for example a catalog query the
    /// operator triggered).
    SetupProbe,
    /// An opt-in live evaluation request.
    LiveTest,
    /// A Laya shadow observation. Never authorizes or routes work and
    /// yields to all foreground capacity.
    ShadowObservation,
}

impl RequestClass {
    pub const fn is_foreground(self) -> bool {
        !matches!(self, Self::ShadowObservation)
    }
}

/// Why a shadow observation was skipped instead of queued. Shadow work
/// is recorded as skipped rather than made to wait: an observation that
/// competes with the agent is worse than one that is absent (PRD 43.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowSkipReason {
    /// Foreground work is already waiting; shadow work never queues
    /// behind it.
    ForegroundQueued,
    /// The shadow allowance (`max_in_flight - foreground_reserved_slots`)
    /// is already consumed by occupied or uncertain shadow slots.
    NoShadowAllowance,
    /// The request's own model capacity identity is at its limit.
    ModelAtLimit,
}

/// Invalid pool limit combination. Surfaced at construction so a bad
/// `.fluzo` mapping fails before any admission decision is made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitsError {
    /// `max_in_flight` must be a positive integer.
    ZeroCapacity,
    /// At least one slot must remain reserved for foreground work, and
    /// the reservation cannot exceed the pool.
    ReservationOutOfRange,
    /// Sanity ceiling on queued waiters; far above the PRD default of 8.
    QueueCapacityTooLarge,
}

/// Validated per-process pool ceilings. Constructed from the typed
/// settings so the PRD defaults live in one place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolLimits {
    pub max_in_flight: u32,
    pub foreground_reserved_slots: u32,
    pub queue_capacity: u32,
}

/// Sanity ceiling only; the PRD default is 8 waiting requests.
pub const MAX_QUEUE_CAPACITY: u32 = 1024;

impl PoolLimits {
    pub const fn new(
        max_in_flight: u32,
        foreground_reserved_slots: u32,
        queue_capacity: u32,
    ) -> Result<Self, LimitsError> {
        if max_in_flight == 0 {
            return Err(LimitsError::ZeroCapacity);
        }
        if foreground_reserved_slots == 0 || foreground_reserved_slots > max_in_flight {
            return Err(LimitsError::ReservationOutOfRange);
        }
        if queue_capacity > MAX_QUEUE_CAPACITY {
            return Err(LimitsError::QueueCapacityTooLarge);
        }
        Ok(Self {
            max_in_flight,
            foreground_reserved_slots,
            queue_capacity,
        })
    }

    /// Slots a shadow observation may ever hold. With the default
    /// one-slot pool this is zero: observations are skipped, not run
    /// in parallel with the agent.
    pub const fn shadow_allowance(&self) -> u32 {
        self.max_in_flight - self.foreground_reserved_slots
    }
}

/// Point-in-time occupancy of one pool. Uncertain slots count as
/// occupied: a request whose remote completion is unconfirmed has not
/// released its capacity (PRD 43.3).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PoolSnapshot {
    pub in_flight: u32,
    pub uncertain: u32,
    pub queued_foreground: u32,
    pub queued_shadow: u32,
}

impl PoolSnapshot {
    pub const fn occupied(&self) -> u32 {
        self.in_flight + self.uncertain
    }

    pub const fn queued_total(&self) -> u32 {
        self.queued_foreground + self.queued_shadow
    }
}

/// Pure admission decision for one request against one pool snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionDecision {
    /// Both the model identity and the pool have capacity; the caller
    /// may dispatch.
    Admit,
    /// No capacity now; the request joins the FIFO foreground queue.
    Queue,
    /// The queue itself is full. A visible `capacity_exhausted` result,
    /// not a retry loop.
    QueueFull,
    /// The shadow observation is skipped with a recorded reason.
    ShadowSkipped(ShadowSkipReason),
}

/// Decide admission for one request.
///
/// `model_at_limit` is the caller's answer for the request's own model
/// capacity identity (aliases of one backend model share this identity
/// rather than multiplying its allowance). The pool rules are applied
/// on top: a request needs both.
///
/// Foreground fairness: while any foreground request is queued, a newly
/// arriving foreground request never jumps ahead, even if a slot is
/// momentarily free.
pub fn decide_admission(
    class: RequestClass,
    model_at_limit: bool,
    pool: &PoolSnapshot,
    limits: &PoolLimits,
) -> AdmissionDecision {
    if !class.is_foreground() {
        if pool.queued_foreground > 0 {
            return AdmissionDecision::ShadowSkipped(ShadowSkipReason::ForegroundQueued);
        }
        if pool.occupied() + pool.queued_shadow >= limits.shadow_allowance() {
            return AdmissionDecision::ShadowSkipped(ShadowSkipReason::NoShadowAllowance);
        }
        if model_at_limit {
            return AdmissionDecision::ShadowSkipped(ShadowSkipReason::ModelAtLimit);
        }
    }
    if pool.queued_foreground > 0 {
        // Fairness first: never overtake a waiting foreground request.
        if pool.queued_total() >= limits.queue_capacity {
            return AdmissionDecision::QueueFull;
        }
        return AdmissionDecision::Queue;
    }
    if model_at_limit || pool.occupied() >= limits.max_in_flight {
        if pool.queued_total() >= limits.queue_capacity {
            return AdmissionDecision::QueueFull;
        }
        return AdmissionDecision::Queue;
    }
    AdmissionDecision::Admit
}

/// The budget dimensions of PRD 43.2 that this MVP enforces per task.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BudgetDimension {
    Turns,
    ActiveSeconds,
    ToolCalls,
    TotalTokens,
    CostMicrounits,
}

/// Machine-readable exhaustion code (PRD 43.1: a non-interactive run
/// returns `budget_exhausted`, never waits for input).
pub const BUDGET_EXHAUSTED_CODE: &str = "budget_exhausted";

/// Which limit was hit, with the numbers behind it. The caller must
/// expose the dimension and measured consumption; neither is inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetExhausted {
    pub dimension: BudgetDimension,
    pub limit: u64,
    pub consumed: u64,
}

impl BudgetExhausted {
    pub const fn code(&self) -> &'static str {
        BUDGET_EXHAUSTED_CODE
    }
}

/// A task's configured budgets, mapped from the typed `[harness]`
/// settings. Monetary enforcement stays disabled unless a limit is
/// configured; absent price data never reports zero consumption as
/// within budget by pretending the limit is infinite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskBudgets {
    pub max_turns: u64,
    pub max_active_seconds: u64,
    pub max_tool_calls: u64,
    pub max_total_tokens: u64,
    pub max_cost_microunits: Option<u64>,
}

impl TaskBudgets {
    pub const fn from_harness(settings: &HarnessSettings) -> Self {
        Self {
            max_turns: settings.max_turns as u64,
            max_active_seconds: settings.max_active_seconds,
            max_tool_calls: settings.max_tool_calls as u64,
            max_total_tokens: settings.max_total_tokens,
            max_cost_microunits: settings.max_cost_microunits,
        }
    }

    pub const fn limit_for(&self, dimension: BudgetDimension) -> Option<u64> {
        match dimension {
            BudgetDimension::Turns => Some(self.max_turns),
            BudgetDimension::ActiveSeconds => Some(self.max_active_seconds),
            BudgetDimension::ToolCalls => Some(self.max_tool_calls),
            BudgetDimension::TotalTokens => Some(self.max_total_tokens),
            BudgetDimension::CostMicrounits => self.max_cost_microunits,
        }
    }
}

/// Measured consumption. Kept separate from the limits so resume
/// preserves consumption independently of a configuration reload.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BudgetConsumption {
    pub turns: u64,
    pub active_seconds: u64,
    pub tool_calls: u64,
    pub total_tokens: u64,
    /// Only meaningful when a monetary budget is configured. Absent
    /// cost data is `None`, never zero.
    pub cost_microunits: Option<u64>,
}

impl BudgetConsumption {
    pub const fn consumed_of(&self, dimension: BudgetDimension) -> Option<u64> {
        match dimension {
            BudgetDimension::Turns => Some(self.turns),
            BudgetDimension::ActiveSeconds => Some(self.active_seconds),
            BudgetDimension::ToolCalls => Some(self.tool_calls),
            BudgetDimension::TotalTokens => Some(self.total_tokens),
            BudgetDimension::CostMicrounits => self.cost_microunits,
        }
    }
}

/// Check every configured dimension against measured consumption.
///
/// Dimensions are evaluated in the fixed `BudgetDimension` order so the
/// reported exhaustion is deterministic when several are exceeded.
/// A dimension with no configured limit (monetary without a budget)
/// cannot be exhausted.
pub fn check_budgets(
    budgets: &TaskBudgets,
    consumed: &BudgetConsumption,
) -> Result<(), BudgetExhausted> {
    for dimension in [
        BudgetDimension::Turns,
        BudgetDimension::ActiveSeconds,
        BudgetDimension::ToolCalls,
        BudgetDimension::TotalTokens,
        BudgetDimension::CostMicrounits,
    ] {
        if let Some(limit) = budgets.limit_for(dimension)
            && let Some(value) = consumed.consumed_of(dimension)
            && value >= limit
        {
            return Err(BudgetExhausted {
                dimension,
                limit,
                consumed: value,
            });
        }
    }
    Ok(())
}

/// Reserve an inference request against the token budget before it is
/// admitted (PRD 43.1: input estimate plus maximum response output
/// must fit the remaining budget).
///
/// `input_estimate` must already be a documented conservative estimate
/// when exact usage is unknown; this function never treats an absent
/// estimate as zero.
pub fn reserve_inference_tokens(
    budgets: &TaskBudgets,
    consumed: &BudgetConsumption,
    input_estimate: u64,
    max_response_output: u64,
) -> Result<(), BudgetExhausted> {
    let limit = budgets.max_total_tokens;
    let projected = consumed
        .total_tokens
        .saturating_add(input_estimate)
        .saturating_add(max_response_output);
    if projected >= limit {
        return Err(BudgetExhausted {
            dimension: BudgetDimension::TotalTokens,
            limit,
            consumed: consumed.total_tokens,
        });
    }
    Ok(())
}

/// Remaining active-time budget, saturating at zero. Queue time,
/// inference time, retry backoff and compaction all consume it; human
/// waits do not, and their exclusion is the caller's accounting duty.
pub fn remaining_active_seconds(budgets: &TaskBudgets, consumed: &BudgetConsumption) -> u64 {
    budgets
        .max_active_seconds
        .saturating_sub(consumed.active_seconds)
}

/// Typed classification of a provider response failure.
///
/// The classification is made from the status alone; `Retry-After` and
/// other rate-limit headers are deliberately not inputs, so no header
/// can schedule a wait (PRD 43.1/43.3). An adapter that recognizes a
/// quota or server-capacity rejection reports it as
/// [`ProviderRejection::RateLimited`] directly: both share the
/// no-automatic-recovery category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderRejection {
    /// HTTP 429 or an explicitly recognized quota/capacity rejection.
    /// Never retry-eligible, no cooldown scheduling, no fallback.
    RateLimited,
    /// A safely classifiable transient failure (service error or
    /// connection interruption). The only retry-eligible category,
    /// and retries still re-enter admission control.
    Transient,
    /// Invalid request, authentication failure or anything not
    /// recognized as transient. Never retried automatically.
    Fatal,
}

impl ProviderRejection {
    pub const fn retry_eligible(self) -> bool {
        matches!(self, Self::Transient)
    }
}

/// Classify an HTTP status for retry policy purposes.
pub const fn classify_status(status: u16) -> ProviderRejection {
    match status {
        429 => ProviderRejection::RateLimited,
        500..=599 => ProviderRejection::Transient,
        _ => ProviderRejection::Fatal,
    }
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod admission_tests;
