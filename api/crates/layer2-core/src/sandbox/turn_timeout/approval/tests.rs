use super::*;

fn budget() -> ApprovalBudget {
    ApprovalBudget::admit(
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: 10_000_000,
        },
        1_000,
    )
    .unwrap()
}

fn binding(id: &str) -> ApprovalOperationBinding {
    ApprovalOperationBinding {
        operation_id: id.into(),
        model_attempt_id: format!("attempt-{id}"),
        provider: "external".into(),
        authority_digest: "c".repeat(64),
    }
}

fn pending(id: &str, start: i64, expiry: i64) -> PendingApprovalReceipt {
    PendingApprovalReceipt {
        binding: binding(id),
        review_id: format!("review-{id}"),
        revision: 1,
        created_at_ms: start,
        expires_at_ms: expiry,
    }
}

fn approved(id: &str, end: i64) -> ResolvedApprovalReceipt {
    ResolvedApprovalReceipt {
        binding: binding(id),
        revision: 2,
        resolved_at_ms: end,
        resolution: ApprovalResolution::Approved,
    }
}

#[test]
fn retained_resolution_without_a_wait_cannot_authorize_reuse_or_refund_a_budget() {
    let registered = budget().register(binding("one"), 1_000).unwrap();
    let mut malformed = serde_json::to_value(registered).unwrap();
    malformed["operations"][0]["resolution"] = "approved".into();
    malformed["operations"][0]["ended_at_ms"] = 1_000.into();
    assert!(serde_json::from_value::<ApprovalBudget>(malformed)
        .unwrap()
        .validate()
        .is_err());
}

#[test]
fn overlap_is_charged_once_and_approved_execution_is_excluded() {
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .register(binding("two"), 1_000)
        .unwrap()
        .pending(&pending("one", 1_000, 901_000), 1_000)
        .unwrap()
        .pending(&pending("two", 51_000, 901_000), 51_000)
        .unwrap();
    assert_eq!(policy.deadline_at_ms(), Some(301_000));
    assert_eq!(policy.consumed_ms(101_000), 100_000);
    let policy = policy
        .resolve(&approved("one", 101_000), 101_000)
        .unwrap()
        .resolve(&approved("two", 151_000), 151_000)
        .unwrap();
    assert!(!policy.is_waiting());
    assert_eq!(policy.consumed_ms(401_000), 150_000);
    assert_eq!(policy.deadline_at_ms(), None);
    let later = policy
        .register(binding("three"), 501_000)
        .unwrap()
        .pending(&pending("three", 501_000, 901_000), 501_000)
        .unwrap();
    assert_eq!(later.deadline_at_ms(), Some(651_000));
    assert_eq!(later.consumed_ms(601_000), 250_000);
}

#[test]
fn late_receipt_and_review_alias_never_refill_the_budget() {
    let first = pending("one", 1_000, 901_000);
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .pending(&first, 351_000)
        .unwrap();
    assert_eq!(policy.deadline_at_ms(), Some(301_000));
    let replay = policy.pending(&first, 451_000).unwrap();
    assert_eq!(replay.observed_at_ms, 451_000);
    assert_eq!(replay.operations, policy.operations);
    assert_eq!(replay.deadline_at_ms(), policy.deadline_at_ms());
    let replacement = PendingApprovalReceipt {
        revision: 2,
        created_at_ms: 351_000,
        expires_at_ms: 951_000,
        review_id: "replacement-review".into(),
        ..first.clone()
    };
    let replaced = policy.pending(&replacement, 451_000).unwrap();
    assert_eq!(replaced.deadline_at_ms(), Some(301_000));
    assert_eq!(replaced.waiting_since_ms(), Some(1_000));
    assert!(replaced.register(binding("later"), 451_000).is_err());
    assert!(replaced.pending(&first, 451_000).is_err());
    let changed_same_revision = PendingApprovalReceipt {
        review_id: "conflict".into(),
        ..replacement
    };
    assert!(replaced.pending(&changed_same_revision, 451_000).is_err());
}

#[test]
fn independent_waits_and_resolution_replays_do_not_clear_each_other() {
    let first = pending("one", 1_000, 901_000);
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .register(binding("two"), 1_000)
        .unwrap()
        .pending(&first, 1_000)
        .unwrap()
        .pending(&pending("two", 1_000, 901_000), 1_000)
        .unwrap();
    let resolution = approved("one", 51_000);
    let policy = policy.resolve(&resolution, 51_000).unwrap();
    assert!(policy.is_waiting());
    assert_eq!(policy.deadline_at_ms(), Some(301_000));
    let replay = policy.resolve(&resolution, 81_000).unwrap();
    assert_eq!(replay.observed_at_ms, 81_000);
    assert_eq!(replay.operations, policy.operations);
    assert_eq!(replay.consumed_ms(51_000), 80_000);
    assert!(policy
        .pending(
            &PendingApprovalReceipt {
                revision: 3,
                ..first
            },
            81_000
        )
        .is_err());
}

#[test]
fn stale_background_and_other_model_attempts_cannot_bind_by_operation_name() {
    let policy = budget().register(binding("one"), 1_000).unwrap();
    assert!(policy
        .pending(&pending("background", 1_000, 901_000), 1_000)
        .is_err());
    let mut wrong = pending("one", 1_000, 901_000);
    wrong.binding.model_attempt_id = "parallel-attempt".into();
    assert!(policy.pending(&wrong, 1_000).is_err());
    assert!(policy.register(wrong.binding, 1_000).is_err());
}

#[test]
fn earlier_provider_expiry_and_bounded_clock_uncertainty_are_conservative() {
    let policy = ApprovalBudget::admit(
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: -1_000,
            offset_max_ms: 1_000,
            valid_until_ms: 901_000,
        },
        1_000,
    )
    .unwrap()
    .register(binding("one"), 1_000)
    .unwrap()
    .pending(&pending("one", 11_000, 101_000), 21_000)
    .unwrap();
    assert_eq!(policy.waiting_since_ms(), Some(10_000));
    assert_eq!(policy.deadline_at_ms(), Some(100_000));
    let approved = policy.resolve(&approved("one", 51_000), 61_000).unwrap();
    assert_eq!(approved.consumed_ms(81_000), 42_000);
    assert!(policy
        .pending(
            &PendingApprovalReceipt {
                revision: 2,
                ..pending("one", 11_000, 201_000)
            },
            902_000
        )
        .is_err());
}

#[test]
fn clock_rollback_does_not_refund_already_observed_wait() {
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .pending(&pending("one", 1_000, 901_000), 101_000)
        .unwrap();
    assert_eq!(policy.consumed_ms(51_000), 100_000);
    assert_eq!(policy.deadline_at_ms(), Some(301_000));
    assert!(policy
        .pending(
            &PendingApprovalReceipt {
                revision: 2,
                ..pending("one", 201_000, 901_000)
            },
            51_000
        )
        .is_err());
}

#[test]
fn restart_retains_spent_budget_and_capacity_never_evicts_obligations() {
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .pending(&pending("one", 1_000, 901_000), 1_000)
        .unwrap()
        .resolve(&approved("one", 101_000), 101_000)
        .unwrap();
    let restarted: ApprovalBudget =
        serde_json::from_slice(&serde_json::to_vec(&policy).unwrap()).unwrap();
    restarted.validate().unwrap();
    assert_eq!(restarted, policy);
    assert_eq!(restarted.consumed_ms(501_000), 100_000);
    let mut full = budget();
    for index in 0..MAX_OPERATIONS {
        full = full
            .register(binding(&format!("op-{index}")), 1_000)
            .unwrap();
    }
    assert!(full.register(binding("overflow"), 1_000).is_err());
    let preserved = full
        .pending(&pending("op-0", 1_000, 901_000), 1_000)
        .unwrap();
    assert_eq!(preserved.deadline_at_ms(), Some(301_000));
}

#[test]
fn receipt_after_resolution_can_shorten_cumulative_budget_without_double_counting() {
    let policy = budget()
        .register(binding("one"), 1_000)
        .unwrap()
        .register(binding("two"), 1_000)
        .unwrap()
        .pending(&pending("one", 101_000, 901_000), 101_000)
        .unwrap()
        .resolve(&approved("one", 201_000), 201_000)
        .unwrap();
    let delayed = policy
        .pending(&pending("two", 51_000, 901_000), 251_000)
        .unwrap();
    assert_eq!(delayed.consumed_ms(251_000), 200_000);
    assert_eq!(delayed.deadline_at_ms(), Some(351_000));
}
