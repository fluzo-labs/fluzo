//! Pure admission and budget rule tests (PRD 43.1-43.3, A04).

use super::*;
use crate::settings::{CapacityPoolSettings, HarnessSettings};

fn limits(max: u32, reserved: u32, queue: u32) -> PoolLimits {
    PoolLimits::new(max, reserved, queue).expect("test limits are valid")
}

fn pool(in_flight: u32, uncertain: u32, qfg: u32, qsh: u32) -> PoolSnapshot {
    PoolSnapshot {
        in_flight,
        uncertain,
        queued_foreground: qfg,
        queued_shadow: qsh,
    }
}

#[test]
fn limits_reject_zero_capacity_and_bad_reservation() {
    assert_eq!(
        PoolLimits::new(0, 1, 8),
        Err(LimitsError::ZeroCapacity),
        "max_in_flight must be a positive integer"
    );
    assert_eq!(
        PoolLimits::new(2, 0, 8),
        Err(LimitsError::ReservationOutOfRange),
        "at least one slot must stay reserved for foreground work"
    );
    assert_eq!(
        PoolLimits::new(2, 3, 8),
        Err(LimitsError::ReservationOutOfRange),
        "the reservation cannot exceed the pool"
    );
    assert_eq!(
        PoolLimits::new(2, 1, MAX_QUEUE_CAPACITY + 1),
        Err(LimitsError::QueueCapacityTooLarge)
    );
}

#[test]
fn defaults_come_from_typed_settings() {
    let from_settings = limits(
        CapacityPoolSettings::default().max_in_flight,
        CapacityPoolSettings::default().foreground_reserved_slots,
        CapacityPoolSettings::default().queue_capacity,
    );
    assert_eq!(from_settings.max_in_flight, 1);
    assert_eq!(from_settings.foreground_reserved_slots, 1);
    assert_eq!(from_settings.queue_capacity, 8);
    assert_eq!(
        from_settings.shadow_allowance(),
        0,
        "the default one-slot pool leaves zero shadow allowance"
    );
}

#[test]
fn empty_pool_admits_foreground_and_shadow_within_allowance() {
    let l = limits(4, 1, 8);
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(0, 0, 0, 0), &l),
        AdmissionDecision::Admit
    );
    assert_eq!(
        decide_admission(RequestClass::Compaction, false, &pool(0, 0, 0, 0), &l),
        AdmissionDecision::Admit,
        "compaction is ordinary foreground work"
    );
    assert_eq!(
        decide_admission(
            RequestClass::ShadowObservation,
            false,
            &pool(0, 0, 0, 0),
            &l
        ),
        AdmissionDecision::Admit,
        "a shadow slot fits inside the 3-slot allowance"
    );
}

#[test]
fn default_pool_skips_shadow_instead_of_competing() {
    let l = limits(1, 1, 8);
    assert_eq!(
        decide_admission(
            RequestClass::ShadowObservation,
            false,
            &pool(0, 0, 0, 0),
            &l
        ),
        AdmissionDecision::ShadowSkipped(ShadowSkipReason::NoShadowAllowance),
        "with one slot and one reserved, the shadow allowance is zero"
    );
}

#[test]
fn shadow_never_queues_behind_foreground() {
    let l = limits(4, 1, 8);
    assert_eq!(
        decide_admission(
            RequestClass::ShadowObservation,
            false,
            &pool(0, 0, 1, 0),
            &l
        ),
        AdmissionDecision::ShadowSkipped(ShadowSkipReason::ForegroundQueued),
        "any queued foreground work skips shadow immediately, even with free capacity"
    );
}

#[test]
fn shadow_allowance_counts_uncertain_shadow_slots() {
    let l = limits(3, 1, 8); // allowance = 2
    assert_eq!(
        decide_admission(
            RequestClass::ShadowObservation,
            false,
            &pool(0, 2, 0, 0),
            &l
        ),
        AdmissionDecision::ShadowSkipped(ShadowSkipReason::NoShadowAllowance),
        "uncertain shadow slots stay charged to the allowance"
    );
    assert_eq!(
        decide_admission(
            RequestClass::ShadowObservation,
            false,
            &pool(1, 0, 0, 1),
            &l
        ),
        AdmissionDecision::ShadowSkipped(ShadowSkipReason::NoShadowAllowance),
        "occupied plus queued shadow fill the allowance"
    );
}

#[test]
fn model_at_limit_skips_shadow_but_queues_foreground() {
    let l = limits(4, 1, 8);
    assert_eq!(
        decide_admission(RequestClass::ShadowObservation, true, &pool(0, 0, 0, 0), &l),
        AdmissionDecision::ShadowSkipped(ShadowSkipReason::ModelAtLimit)
    );
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, true, &pool(0, 0, 0, 0), &l),
        AdmissionDecision::Queue,
        "a model-limited foreground request waits rather than being dropped"
    );
}

#[test]
fn pool_ceiling_queues_foreground_and_uncertain_counts_as_occupied() {
    let l = limits(2, 1, 8);
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(2, 0, 0, 0), &l),
        AdmissionDecision::Queue
    );
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(1, 1, 0, 0), &l),
        AdmissionDecision::Queue,
        "an uncertain slot has not released capacity"
    );
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(1, 0, 0, 0), &l),
        AdmissionDecision::Admit
    );
}

#[test]
fn foreground_fairness_never_jumps_the_queue() {
    let l = limits(2, 1, 8);
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(1, 0, 1, 0), &l),
        AdmissionDecision::Queue,
        "a free slot does not let a new arrival overtake a waiting request"
    );
    assert_eq!(
        decide_admission(RequestClass::SetupProbe, false, &pool(0, 0, 1, 0), &l),
        AdmissionDecision::Queue,
        "all foreground classes are FIFO-equal"
    );
}

#[test]
fn queue_capacity_produces_visible_exhaustion() {
    let l = limits(1, 1, 2);
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(1, 0, 2, 0), &l),
        AdmissionDecision::QueueFull
    );
    assert_eq!(
        decide_admission(RequestClass::AgentTurn, false, &pool(1, 0, 1, 1), &l),
        AdmissionDecision::QueueFull,
        "shadow waiters count toward the queue bound"
    );
}

#[test]
fn budgets_map_from_harness_settings() {
    let budgets = TaskBudgets::from_harness(&HarnessSettings::default());
    assert_eq!(budgets.max_turns, 30);
    assert_eq!(budgets.max_active_seconds, 1200);
    assert_eq!(budgets.max_tool_calls, 100);
    assert_eq!(budgets.max_total_tokens, 200_000);
    assert_eq!(budgets.max_cost_microunits, None);
}

#[test]
fn exhaustion_reports_dimension_limit_and_consumption() {
    let budgets = TaskBudgets {
        max_turns: 3,
        max_active_seconds: 100,
        max_tool_calls: 5,
        max_total_tokens: 1000,
        max_cost_microunits: Some(500),
    };
    let consumed = BudgetConsumption {
        turns: 3,
        active_seconds: 40,
        tool_calls: 2,
        total_tokens: 300,
        cost_microunits: Some(10),
    };
    assert_eq!(
        check_budgets(&budgets, &consumed),
        Err(BudgetExhausted {
            dimension: BudgetDimension::Turns,
            limit: 3,
            consumed: 3,
        }),
        "reaching the limit exhausts it"
    );
    assert_eq!(
        check_budgets(
            &budgets,
            &BudgetConsumption {
                turns: 2,
                ..consumed
            }
        ),
        Ok(()),
        "two turns is still within a limit of three"
    );
}

#[test]
fn exhaustion_order_is_deterministic() {
    let budgets = TaskBudgets {
        max_turns: 1,
        max_active_seconds: 1,
        max_tool_calls: 1,
        max_total_tokens: 1,
        max_cost_microunits: Some(1),
    };
    let consumed = BudgetConsumption {
        turns: 9,
        active_seconds: 9,
        tool_calls: 9,
        total_tokens: 9,
        cost_microunits: Some(9),
    };
    assert_eq!(
        check_budgets(&budgets, &consumed),
        Err(BudgetExhausted {
            dimension: BudgetDimension::Turns,
            limit: 1,
            consumed: 9,
        }),
        "the fixed dimension order decides which exhaustion is reported"
    );
}

#[test]
fn monetary_budget_without_data_cannot_be_exhausted() {
    let budgets = TaskBudgets {
        max_turns: 10,
        max_active_seconds: 10,
        max_tool_calls: 10,
        max_total_tokens: 10_000,
        max_cost_microunits: None,
    };
    let consumed = BudgetConsumption {
        turns: 1,
        ..BudgetConsumption::default()
    };
    assert_eq!(check_budgets(&budgets, &consumed), Ok(()));
    assert_eq!(
        consumed.cost_microunits, None,
        "absent cost data is not zero consumption"
    );
}

#[test]
fn token_reservation_requires_estimate_plus_max_output_to_fit() {
    let budgets = TaskBudgets {
        max_turns: 10,
        max_active_seconds: 100,
        max_tool_calls: 10,
        max_total_tokens: 1000,
        max_cost_microunits: None,
    };
    let consumed = BudgetConsumption {
        total_tokens: 500,
        ..BudgetConsumption::default()
    };
    assert_eq!(
        reserve_inference_tokens(&budgets, &consumed, 300, 199),
        Ok(()),
        "500 + 300 + 199 = 999 fits below 1000"
    );
    assert_eq!(
        reserve_inference_tokens(&budgets, &consumed, 300, 200),
        Err(BudgetExhausted {
            dimension: BudgetDimension::TotalTokens,
            limit: 1000,
            consumed: 500,
        }),
        "reaching the budget with the full reservation blocks the request"
    );
    assert_eq!(
        reserve_inference_tokens(&budgets, &consumed, u64::MAX, u64::MAX),
        Err(BudgetExhausted {
            dimension: BudgetDimension::TotalTokens,
            limit: 1000,
            consumed: 500,
        }),
        "saturating arithmetic cannot overflow past the limit"
    );
}

#[test]
fn remaining_active_time_saturates() {
    let budgets = TaskBudgets {
        max_turns: 10,
        max_active_seconds: 100,
        max_tool_calls: 10,
        max_total_tokens: 10_000,
        max_cost_microunits: None,
    };
    assert_eq!(
        remaining_active_seconds(&budgets, &BudgetConsumption::default()),
        100
    );
    assert_eq!(
        remaining_active_seconds(
            &budgets,
            &BudgetConsumption {
                active_seconds: 250,
                ..BudgetConsumption::default()
            }
        ),
        0
    );
}

#[test]
fn rate_limit_and_capacity_rejections_are_never_retry_eligible() {
    assert_eq!(classify_status(429), ProviderRejection::RateLimited);
    assert!(!classify_status(429).retry_eligible());
    assert_eq!(classify_status(503), ProviderRejection::Transient);
    assert!(classify_status(503).retry_eligible());
    assert_eq!(classify_status(401), ProviderRejection::Fatal);
    assert_eq!(classify_status(400), ProviderRejection::Fatal);
    assert_eq!(
        classify_status(200),
        ProviderRejection::Fatal,
        "not a failure"
    );
    assert!(!ProviderRejection::RateLimited.retry_eligible());
    assert!(!ProviderRejection::Fatal.retry_eligible());
}

#[test]
fn exhaustion_carries_the_machine_readable_code() {
    let exhausted = BudgetExhausted {
        dimension: BudgetDimension::ToolCalls,
        limit: 100,
        consumed: 100,
    };
    assert_eq!(exhausted.code(), "budget_exhausted");
}
