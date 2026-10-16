//! Per-process admission controller tests (PRD 43.3, A04).

use super::*;

fn pool_id(name: &str) -> PoolId {
    PoolId(name.to_owned())
}

fn model_id(name: &str) -> ModelId {
    ModelId(name.to_owned())
}

fn models(entries: &[(&str, u32)]) -> BTreeMap<ModelId, u32> {
    entries
        .iter()
        .map(|(name, limit)| (model_id(name), *limit))
        .collect()
}

fn request(class: RequestClass, pool: &str, model: &str) -> AdmissionRequest {
    AdmissionRequest {
        class,
        pool: pool_id(pool),
        model: model_id(model),
        input_estimate_tokens: 100,
        max_response_output_tokens: 50,
        remaining_task_seconds: 3600,
        budgets: None,
        consumed: None,
    }
}

fn controller_with(max: u32, reserved: u32, queue: u32, timeout_ms: u64) -> AdmissionController {
    let controller = AdmissionController::new();
    controller
        .register_pool(
            pool_id("shared"),
            PoolLimits::new(max, reserved, queue).unwrap(),
            timeout_ms,
            models(&[("coder", max), ("shadow", max)]),
        )
        .unwrap();
    controller
}

#[test]
fn admits_up_to_the_pool_ceiling_then_queues() {
    let c = controller_with(2, 1, 8, 30_000);
    let first = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    let second = c.try_admit(&request(RequestClass::Compaction, "shared", "coder"), 0);
    assert!(matches!(first, Admission::Admitted(_)));
    assert!(matches!(second, Admission::Admitted(_)));
    let third = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(
        matches!(third, Admission::Queued { position: 1, .. }),
        "third concurrent request must wait"
    );
}

#[test]
fn queue_full_is_a_visible_typed_denial() {
    let c = controller_with(1, 1, 1, 30_000);
    let admitted = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(admitted, Admission::Admitted(_)));
    assert!(matches!(
        c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0),
        Admission::Queued { .. }
    ));
    assert_eq!(
        c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0),
        Admission::Denied(AdmissionDenied::CapacityExhausted)
    );
}

#[test]
fn freed_capacity_hands_out_fifo() {
    let c = controller_with(1, 1, 8, 30_000);
    let slot1 = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let a = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 10);
    let b = c.try_admit(&request(RequestClass::Compaction, "shared", "coder"), 20);
    let (ta, tb) = match (a, b) {
        (Admission::Queued { ticket: a, .. }, Admission::Queued { ticket: b, .. }) => (a, b),
        other => panic!("expected queued, got {other:?}"),
    };
    assert_eq!(
        c.admit_next(&pool_id("shared"), 30),
        None,
        "the pool is still full while slot1 is occupied"
    );
    c.release(slot1, ReleaseKind::Completed);
    let admitted_a = c
        .admit_next(&pool_id("shared"), 40)
        .expect("first waiter gets the freed slot");
    assert_eq!(admitted_a.0, ta);
    assert_eq!(
        c.admit_next(&pool_id("shared"), 40),
        None,
        "one-slot pool cannot admit a second waiter"
    );
    c.release(admitted_a.1, ReleaseKind::Completed);
    let admitted_b = c
        .admit_next(&pool_id("shared"), 50)
        .expect("second waiter proceeds");
    assert_eq!(admitted_b.0, tb);
}

#[test]
fn queue_deadline_is_bounded_by_remaining_task_time() {
    let c = controller_with(1, 1, 8, 30_000);
    let blocker = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(blocker, Admission::Admitted(_)));
    let mut req = request(RequestClass::AgentTurn, "shared", "coder");
    req.remaining_task_seconds = 5;
    match c.try_admit(&req, 1_000) {
        Admission::Queued { deadline_ms, .. } => {
            assert_eq!(
                deadline_ms, 6_000,
                "30s pool timeout must shrink to the 5s remaining task budget"
            );
        }
        other => panic!("expected queued, got {other:?}"),
    }
}

#[test]
fn zero_remaining_task_time_denies_without_queueing() {
    let c = controller_with(1, 1, 8, 30_000);
    let blocker = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(blocker, Admission::Admitted(_)));
    let mut req = request(RequestClass::AgentTurn, "shared", "coder");
    req.remaining_task_seconds = 0;
    assert_eq!(
        c.try_admit(&req, 1_000),
        Admission::Denied(AdmissionDenied::CapacityWaitTimeout),
        "a wait that would already be expired never enters the queue"
    );
}

#[test]
fn expire_times_out_only_dead_waiters() {
    let c = controller_with(1, 1, 8, 1_000);
    let _blocker = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    let early = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 100) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued, got {other:?}"),
    };
    let late = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 900) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued, got {other:?}"),
    };
    assert_eq!(c.expire(1_200), vec![early]);
    assert_eq!(c.expire(2_000), vec![late]);
    assert!(c.expire(3_000).is_empty());
}

#[test]
fn cancelled_queue_entry_is_removed_without_dispatch() {
    let c = controller_with(1, 1, 8, 30_000);
    let blocker = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let doomed = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 10) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued, got {other:?}"),
    };
    assert!(c.cancel_queued(doomed));
    assert!(!c.cancel_queued(doomed), "already removed");
    c.release(blocker, ReleaseKind::Completed);
    assert_eq!(
        c.admit_next(&pool_id("shared"), 20),
        None,
        "the cancelled request must not be handed capacity"
    );
}

#[test]
fn uncertain_blocks_and_reconcile_frees() {
    let c = controller_with(1, 1, 8, 30_000);
    let slot = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    c.release(slot, ReleaseKind::Uncertain);
    let waiter = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 10) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("uncertain slot must block new admission, got {other:?}"),
    };
    assert_eq!(
        c.admit_next(&pool_id("shared"), 15),
        None,
        "uncertain occupancy is not free capacity"
    );
    assert!(
        c.reconcile(slot, ReconcileEvidence::Completed),
        "reconciling an uncertain slot frees it"
    );
    let admitted = c
        .admit_next(&pool_id("shared"), 20)
        .expect("waiter gets the reconciled slot");
    assert_eq!(admitted.0, waiter);
    assert!(
        !c.reconcile(slot, ReconcileEvidence::Completed),
        "already reconciled"
    );
}

#[test]
fn reduced_limit_drains_without_cancelling() {
    let c = controller_with(2, 1, 8, 30_000);
    let a = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let b = match c.try_admit(&request(RequestClass::Compaction, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    c.apply_limits(&pool_id("shared"), PoolLimits::new(1, 1, 8).unwrap())
        .unwrap();
    assert_eq!(
        c.view(&pool_id("shared"), 0).unwrap().in_flight,
        2,
        "lowering the limit must not cancel admitted requests"
    );
    let waiter = match c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 10) {
        Admission::Queued { ticket, .. } => ticket,
        other => panic!("expected queued above the new ceiling, got {other:?}"),
    };
    c.release(a, ReleaseKind::Completed);
    assert_eq!(
        c.admit_next(&pool_id("shared"), 20),
        None,
        "one admitted request still equals the new ceiling of one"
    );
    c.release(b, ReleaseKind::Completed);
    let admitted = c
        .admit_next(&pool_id("shared"), 30)
        .expect("pool drains below the new limit");
    assert_eq!(admitted.0, waiter);
}

#[test]
fn aliases_share_one_capacity_identity() {
    let c = controller_with(4, 1, 8, 30_000);
    // Both aliases resolve to the same ModelId upstream; the controller
    // sees one identity and one counter.
    let a = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    let b = c.try_admit(&request(RequestClass::Compaction, "shared", "coder"), 0);
    assert!(matches!(a, Admission::Admitted(_)));
    assert!(matches!(b, Admission::Admitted(_)));
    // The model limit (4 here) is shared, not multiplied per alias.
    let view = c.view(&pool_id("shared"), 0).unwrap();
    assert_eq!(view.models.get("coder").unwrap().in_flight, 2);
    assert_eq!(view.models.get("coder").unwrap().limit, 4);
}

#[test]
fn model_ceiling_queues_and_holds_the_pool_fifo() {
    let controller = AdmissionController::new();
    controller
        .register_pool(
            pool_id("shared"),
            PoolLimits::new(4, 1, 8).unwrap(),
            30_000,
            models(&[("coder", 1), ("other", 4)]),
        )
        .unwrap();
    let a = controller.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(a, Admission::Admitted(_)));
    assert!(
        matches!(
            controller.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0),
            Admission::Queued { .. }
        ),
        "model ceiling binds even with pool capacity free"
    );
    assert!(
        matches!(
            controller.try_admit(&request(RequestClass::AgentTurn, "shared", "other"), 0),
            Admission::Queued { position: 2, .. }
        ),
        "strict FIFO: a later request queues behind the waiting coder instead \
         of overtaking it on a different model"
    );
}

#[test]
fn shadow_is_skipped_on_default_pool_through_the_controller() {
    let c = controller_with(1, 1, 8, 30_000);
    assert_eq!(
        c.try_admit(
            &request(RequestClass::ShadowObservation, "shared", "shadow"),
            0
        ),
        Admission::Denied(AdmissionDenied::ShadowSkipped {
            reason: ShadowSkipReason::NoShadowAllowance,
        })
    );
}

#[test]
fn budget_exhaustion_denies_before_capacity_is_considered() {
    let c = controller_with(4, 1, 8, 30_000);
    let mut req = request(RequestClass::AgentTurn, "shared", "coder");
    req.budgets = Some(TaskBudgets {
        max_turns: 2,
        max_active_seconds: 100,
        max_tool_calls: 10,
        max_total_tokens: 10_000,
        max_cost_microunits: None,
    });
    req.consumed = Some(BudgetConsumption {
        turns: 2,
        ..BudgetConsumption::default()
    });
    assert_eq!(
        c.try_admit(&req, 0),
        Admission::Denied(AdmissionDenied::BudgetExhausted(BudgetExhausted {
            dimension: fluzo_core::admission::BudgetDimension::Turns,
            limit: 2,
            consumed: 2,
        }))
    );
}

#[test]
fn token_reservation_denies_an_oversized_request() {
    let c = controller_with(4, 1, 8, 30_000);
    let mut req = request(RequestClass::AgentTurn, "shared", "coder");
    req.budgets = Some(TaskBudgets {
        max_turns: 10,
        max_active_seconds: 100,
        max_tool_calls: 10,
        max_total_tokens: 1_000,
        max_cost_microunits: None,
    });
    req.consumed = Some(BudgetConsumption {
        total_tokens: 900,
        ..BudgetConsumption::default()
    });
    // 900 + 100 input + 50 output reaches the budget.
    assert_eq!(
        c.try_admit(&req, 0),
        Admission::Denied(AdmissionDenied::BudgetExhausted(BudgetExhausted {
            dimension: fluzo_core::admission::BudgetDimension::TotalTokens,
            limit: 1_000,
            consumed: 900,
        }))
    );
}

#[test]
fn budgets_must_be_supplied_as_a_pair() {
    let c = controller_with(4, 1, 8, 30_000);
    let mut req = request(RequestClass::AgentTurn, "shared", "coder");
    req.budgets = Some(TaskBudgets {
        max_turns: 10,
        max_active_seconds: 100,
        max_tool_calls: 10,
        max_total_tokens: 1_000,
        max_cost_microunits: None,
    });
    assert_eq!(
        c.try_admit(&req, 0),
        Admission::Denied(AdmissionDenied::BudgetsIncomplete)
    );
}

#[test]
fn unknown_pool_and_model_are_typed_denials() {
    let c = controller_with(1, 1, 8, 30_000);
    assert_eq!(
        c.try_admit(&request(RequestClass::AgentTurn, "nope", "coder"), 0),
        Admission::Denied(AdmissionDenied::UnknownPool)
    );
    assert_eq!(
        c.try_admit(&request(RequestClass::AgentTurn, "shared", "ghost"), 0),
        Admission::Denied(AdmissionDenied::UnknownModel)
    );
}

#[test]
fn two_controllers_never_coordinate() {
    // Same pool name, same model, same limits: two processes' worth of
    // admission. Each admits its own ceiling; nothing combines or
    // restricts the other (A04: oversubscription is the user's
    // responsibility, not a coordination failure).
    let one = controller_with(2, 1, 8, 30_000);
    let two = controller_with(2, 1, 8, 30_000);
    let a = one.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    let b = two.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(a, Admission::Admitted(_)));
    assert!(matches!(b, Admission::Admitted(_)));
    let c = one.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    let d = two.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(c, Admission::Admitted(_)));
    assert!(matches!(d, Admission::Admitted(_)));
    assert!(matches!(
        one.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0),
        Admission::Queued { .. }
    ));
    assert!(matches!(
        two.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0),
        Admission::Queued { .. }
    ));
    // Four concurrent slots across two processes with max 2 each: the
    // expected non-coordination, not a bug.
    assert_eq!(
        one.view(&pool_id("shared"), 0).unwrap().in_flight
            + two.view(&pool_id("shared"), 0).unwrap().in_flight,
        4
    );
}

#[test]
fn view_reports_queue_wait_separately() {
    let c = controller_with(1, 1, 8, 30_000);
    let blocker = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 0);
    assert!(matches!(blocker, Admission::Admitted(_)));
    let queued = c.try_admit(&request(RequestClass::AgentTurn, "shared", "coder"), 500);
    assert!(matches!(queued, Admission::Queued { .. }));
    let view = c.view(&pool_id("shared"), 2_500).unwrap();
    assert_eq!(view.queued_foreground, 1);
    assert_eq!(
        view.oldest_queue_wait_ms, 2_000,
        "queue duration is visible independently of inference time"
    );
}

#[test]
fn slot_identity_correlates_back_to_the_request() {
    let c = controller_with(2, 1, 8, 30_000);
    let slot = match c.try_admit(&request(RequestClass::Compaction, "shared", "coder"), 0) {
        Admission::Admitted(slot) => slot,
        other => panic!("expected admission, got {other:?}"),
    };
    let (ticket, class) = c.slot_info(slot).expect("slot is tracked");
    assert_eq!(
        ticket,
        Ticket(slot.0),
        "admission ties the slot to its ticket"
    );
    assert_eq!(class, RequestClass::Compaction);
    c.release(slot, ReleaseKind::Completed);
    assert!(
        c.slot_info(slot).is_none(),
        "released slots are no longer tracked"
    );
}

#[test]
fn registration_rejects_bad_configuration() {
    let c = AdmissionController::new();
    assert!(
        PoolLimits::new(0, 1, 8).is_err(),
        "zero capacity is invalid"
    );
    let limits = PoolLimits::new(2, 1, 8).unwrap();
    assert!(matches!(
        c.register_pool(pool_id("shared"), limits, 0, models(&[("coder", 1)])),
        Err(RegisterError::Limits(_))
    ));
    c.register_pool(pool_id("shared"), limits, 30_000, models(&[("coder", 1)]))
        .unwrap();
    assert!(matches!(
        c.register_pool(pool_id("shared"), limits, 30_000, models(&[("x", 1)])),
        Err(RegisterError::DuplicatePool)
    ));
    assert!(matches!(
        c.register_pool(pool_id("other"), limits, 30_000, models(&[("x", 0)])),
        Err(RegisterError::ZeroModelLimit)
    ));
}
