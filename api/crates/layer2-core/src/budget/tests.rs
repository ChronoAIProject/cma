use super::*;

// Dimensionless fixture units; these are neither prices nor configured allowances.
fn policy() -> BudgetPolicy {
    let unit = u64::from(u8::MAX);
    BudgetPolicy {
        owner_day_limit: unit * unit,
        turn_limit: unit,
        max_calls_per_turn: unit,
        day_timezone: "UTC".into(),
        input_token_units: 1,
        output_token_units: 1,
        max_unobserved_units: 1,
        usage_timeout_seconds: 60,
    }
}

#[test]
fn reservations_exhaust_an_owner_without_mutating_a_refusal() {
    let policy = policy();
    let mut owner = OwnerBudget::default();
    for _ in 0..policy.owner_day_limit / policy.turn_limit {
        owner.reserve(&policy, 0).unwrap();
    }
    let before = owner.clone();
    assert_eq!(owner.reserve(&policy, 0), Err(BudgetError::OwnerExhausted));
    assert_eq!(owner, before);
}

#[test]
fn rollover_preserves_every_unresolved_reservation() {
    let policy = policy();
    let mut owner = OwnerBudget::default();
    owner.reserve(&policy, 0).unwrap();
    owner.spent = policy.turn_limit;
    owner.advance(&policy, 86_400).unwrap();
    assert_eq!(owner.spent, 0);
    assert_eq!(owner.reserved, policy.turn_limit);
    let before = owner.clone();
    assert_eq!(owner.advance(&policy, 0), Err(BudgetError::Unavailable));
    assert_eq!(before, owner);
}

#[test]
fn uncertain_terminal_charges_the_full_allowance() {
    let policy = policy();
    let mut owner = OwnerBudget::default();
    owner.reserve(&policy, 0).unwrap();
    let mut turn = TurnReservation::new(policy.clone(), 0);
    turn.observe(1, 1, 1).unwrap();
    owner.settle(&turn, 1).unwrap();
    assert_eq!(owner.reserved, 0);
    assert_eq!(owner.spent, policy.turn_limit);
    let before = owner.clone();
    assert_eq!(owner.settle(&turn, 1), Err(BudgetError::Unavailable));
    assert_eq!(owner, before);
}

#[test]
fn verified_final_usage_can_release_unused_reservation() {
    let policy = policy();
    let mut owner = OwnerBudget::default();
    owner.reserve(&policy, 0).unwrap();
    let mut turn = TurnReservation::new(policy, 0);
    turn.observe(1, 1, 1).unwrap();
    turn.final_usage_known = true;
    owner.settle(&turn, 1).unwrap();
    assert_eq!(owner.spent, turn.observed);
    assert_eq!(owner.reserved, 0);
}

#[test]
fn lag_headroom_stops_before_observed_exhaustion_and_stays_stopped() {
    let policy = policy();
    let mut turn = TurnReservation::new(policy.clone(), 0);
    turn.observe(policy.turn_limit - policy.max_unobserved_units - 1, 0, 1)
        .unwrap();
    assert_eq!(turn.stop_reason(1), None);
    turn.observe(policy.turn_limit - policy.max_unobserved_units, 0, 2)
        .unwrap();
    assert_eq!(turn.stop_reason(2), Some(BudgetError::TurnExhausted));
    assert_eq!(turn.observe(0, 0, 3), Err(BudgetError::Unavailable));
    assert_eq!(turn.stop_reason(3), Some(BudgetError::TurnExhausted));
}

#[test]
fn unavailable_stale_and_overflowed_usage_never_looks_free() {
    let policy = policy();
    let mut turn = TurnReservation::new(policy.clone(), 1);
    assert_eq!(turn.stop_reason(0), Some(BudgetError::Unavailable));
    assert_eq!(
        turn.stop_reason(policy.usage_timeout_seconds + 2),
        Some(BudgetError::Unavailable)
    );
    assert_eq!(
        turn.observe(u64::MAX, u64::MAX, 2),
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        policy.require_turn_headroom(u64::MAX),
        Err(BudgetError::TurnExhausted)
    );
}

#[test]
fn policy_scope_is_explicit_and_invalid_settings_fail_closed() {
    let owner = SubjectId("budget-test-owner".into());
    let other = SubjectId("budget-other-owner".into());
    let mut settings = BudgetSettings {
        enabled: true,
        platform: Some(PlatformBudgetPolicy {
            period_cap: u64::from(u32::MAX),
            period_start: "2026-11-01T00:00:00Z".into(),
            period_end: "2026-12-01T00:00:00Z".into(),
            warning_percent: 80,
            stop_percent: 100,
            protected_reserve: 0,
            lag_allowance: 0,
            cloud: None,
        }),
        default_policy: None,
        request_models: Vec::new(),
        owners: vec![OwnerBudgetPolicy {
            owner: owner.clone(),
            policy: policy(),
        }],
    };
    settings.validate().unwrap();
    assert!(settings.for_owner(&owner).is_some());
    assert!(settings.for_owner(&other).is_none());
    settings.owners.push(settings.owners[0].clone());
    assert_eq!(settings.validate(), Err(BudgetError::InvalidPolicy));
    let mut policy = policy();
    policy.max_unobserved_units = policy.turn_limit;
    assert_eq!(policy.validate(), Err(BudgetError::InvalidPolicy));
}

#[test]
fn late_usage_increases_outstanding_liability_before_another_admission() {
    let policy = policy();
    let mut owner = OwnerBudget::default();
    owner.reserve(&policy, 0).unwrap();
    let mut turn = TurnReservation::new(policy.clone(), 0);
    turn.observe(policy.owner_day_limit, 0, 1).unwrap();
    owner.retain_observed_liability(&mut turn).unwrap();
    assert!(owner.reserved > policy.owner_day_limit);
    assert_eq!(owner.reserve(&policy, 1), Err(BudgetError::OwnerExhausted));
    assert_eq!(turn.stop_reason(1), Some(BudgetError::TurnExhausted));
}

#[test]
fn changing_valuation_cannot_reinterpret_outstanding_liability() {
    let original = policy();
    let mut owner = OwnerBudget::default();
    owner.reserve(&original, 0).unwrap();
    let mut changed = original.clone();
    changed.input_token_units += 1;
    assert_eq!(owner.reserve(&changed, 0), Err(BudgetError::Unavailable));
    let mut turn = TurnReservation::new(original.clone(), 0);
    assert_eq!(turn.revalidate(&changed), Err(BudgetError::Unavailable));
    changed = original;
    turn.observe(changed.turn_limit / 2, 0, 1).unwrap();
    changed.turn_limit /= 2;
    assert_eq!(turn.revalidate(&changed), Err(BudgetError::TurnExhausted));
}

#[test]
fn admin_json_numbers_must_be_exact_in_browser_clients() {
    let mut policy = policy();
    policy.owner_day_limit = 1_u64 << 53;
    assert_eq!(policy.validate(), Err(BudgetError::InvalidPolicy));
}

#[test]
fn platform_exhaustion_guidance_holds_for_a_refused_allowance_and_for_a_stop() {
    assert_eq!(
        BudgetError::PlatformExhausted.code(),
        "platform_budget_exhausted"
    );
    let guidance = BudgetError::PlatformExhausted.guidance();
    assert_eq!(
        guidance,
        "The platform's spend limit for this period does not allow new work right now. \
         Completed work is preserved. After the limit is raised, send a new message to \
         continue; this turn will not replay automatically."
    );
    // A refused allowance pauses nothing; the pause reason text says so when
    // a Sandbox is actually paused.
    assert!(!guidance.contains("paused"));
}
