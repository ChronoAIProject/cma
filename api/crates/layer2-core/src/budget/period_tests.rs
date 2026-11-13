use super::*;

const START: u64 = 1_700_000_000;
const END: u64 = 1_702_592_000;

fn policy() -> PlatformBudgetPolicy {
    PlatformBudgetPolicy {
        period_cap: 1_000_000,
        period_start: "2023-11-14T22:13:20Z".into(),
        period_end: "2023-12-14T22:13:20Z".into(),
        warning_percent: 80,
        stop_percent: 100,
        protected_reserve: 50_000,
        lag_allowance: 50_000,
        cloud: Some(CloudSpendPolicy {
            run_rate_units_per_day: 86_400,
            max_observation_age_seconds: 3_600,
        }),
    }
}

fn observation(units: u64, observed_through: u64) -> CloudObservation {
    CloudObservation {
        observed_through,
        units,
        recorded_at: observed_through,
        settings_revision: 1,
    }
}

/// An open ledger whose cloud source was observed at exactly `now` with zero
/// units, so committed equals the model figures alone.
fn observed_at(now: u64) -> PlatformPeriodLedger {
    let mut ledger = PlatformPeriodLedger::open(&policy(), 1).unwrap();
    ledger.record_cloud(observation(0, now)).unwrap();
    ledger
}

fn reasons(closures: &[Closure]) -> Vec<ClosureReason> {
    closures.iter().map(|closure| closure.reason).collect()
}

#[test]
fn levels_follow_the_spec_formula() {
    let policy = policy();
    policy.validate().unwrap();
    assert_eq!(
        policy.levels().unwrap(),
        SpendLevels {
            warning: 800_000,
            stop: 1_000_000,
            disposable: 900_000,
        }
    );
    assert_eq!(policy.period_bounds().unwrap(), (START, END));
    for cloud in [
        CloudSpendPolicy {
            run_rate_units_per_day: 0,
            max_observation_age_seconds: 1,
        },
        CloudSpendPolicy {
            run_rate_units_per_day: 1,
            max_observation_age_seconds: 0,
        },
        CloudSpendPolicy {
            run_rate_units_per_day: 1 << 53,
            max_observation_age_seconds: 1,
        },
    ] {
        let invalid = PlatformBudgetPolicy {
            cloud: Some(cloud),
            ..policy.clone()
        };
        assert_eq!(invalid.validate(), Err(BudgetError::InvalidPolicy));
    }
}

#[test]
fn cloud_projection_rounds_up_and_goes_stale() {
    let t0 = START + 100;
    let mut ledger = PlatformPeriodLedger::open(&policy(), 1).unwrap();
    ledger.record_cloud(observation(10, t0)).unwrap();
    assert_eq!(ledger.cloud_committed(&policy(), t0), Ok(10));
    assert_eq!(ledger.cloud_committed(&policy(), t0 + 1), Ok(11));
    let mut slow = policy();
    slow.cloud.as_mut().unwrap().run_rate_units_per_day = 1;
    assert_eq!(ledger.cloud_committed(&slow, t0 + 1), Ok(11));
    assert_eq!(ledger.cloud_committed(&policy(), t0 + 3_600), Ok(3_610));
    assert_eq!(
        ledger.cloud_committed(&policy(), t0 + 3_601),
        Err(BudgetError::Unavailable)
    );
    assert_eq!(
        ledger.committed(&policy(), t0 + 3_601),
        Err(BudgetError::Unavailable)
    );
}

#[test]
fn admission_mirrors_reserved_and_enters_warning_once() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    assert_eq!(ledger.admit(&policy(), 1, 500_000, now), Ok(vec![]));
    assert_eq!(ledger.reserved, 500_000);
    let entered = ledger.admit(&policy(), 1, 300_000, now).unwrap();
    assert_eq!(reasons(&entered), vec![ClosureReason::SpendWarning]);
    assert_eq!(entered[0].since, now);
    assert_eq!(entered[0].settings_revision, 1);
    assert_eq!(entered[0].committed_at_entry, 800_000);
    assert_eq!(ledger.admit(&policy(), 1, 50_000, now), Ok(vec![]));
    assert_eq!(reasons(&ledger.closures), vec![ClosureReason::SpendWarning]);
    assert_eq!(ledger.reserved, 850_000);
    assert!(ledger.closed_for_registration());
    assert!(!ledger.closed_for_commitments());
}

#[test]
fn an_allowance_that_does_not_fit_is_refused_without_entering_the_stop() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    let before = ledger.clone();
    let refusal = ledger.admit(&policy(), 3, 900_001, now).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::PlatformExhausted);
    assert!(refusal.closures.is_empty());
    assert_eq!(ledger, before);
    // Late usage raised committed past the warning level outside any
    // admission: the refusal carries the missing warning, never the stop.
    ledger.retain(850_000).unwrap();
    let before = ledger.clone();
    let refusal = ledger.admit(&policy(), 3, 50_001, now).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::PlatformExhausted);
    assert_eq!(
        reasons(&refusal.closures),
        vec![ClosureReason::SpendWarning]
    );
    assert!(refusal.closures.iter().all(|closure| closure.since == now
        && closure.settings_revision == 3
        && closure.committed_at_entry == 850_000));
    assert_eq!(ledger, before);
    assert!(!ledger.closed_for_commitments());
    // Below disposable a zero liability is admitted, and so is an allowance
    // that exactly reaches it.
    assert_eq!(
        ledger.admit(&policy(), 3, 0, now).map(|c| reasons(&c)),
        Ok(vec![ClosureReason::SpendWarning])
    );
    assert_eq!(ledger.admit(&policy(), 3, 50_000, now), Ok(vec![]));
    assert_eq!(ledger.reserved, 900_000);
    assert_eq!(reasons(&ledger.closures), vec![ClosureReason::SpendWarning]);
}

#[test]
fn a_check_at_or_past_disposable_is_refused_and_enters_the_stop() {
    let now = START + 100;
    let mut below = observed_at(now);
    below.retain(899_999).unwrap();
    assert!(below.admit(&policy(), 3, 0, now).is_ok());
    let mut ledger = observed_at(now);
    ledger.retain(900_000).unwrap();
    let before = ledger.clone();
    for liability in [0, 1] {
        let refusal = ledger.admit(&policy(), 3, liability, now).unwrap_err();
        assert_eq!(refusal.reason, BudgetError::PlatformExhausted);
        assert_eq!(
            reasons(&refusal.closures),
            vec![ClosureReason::SpendStop, ClosureReason::SpendWarning]
        );
        assert!(refusal.closures.iter().all(|closure| closure.since == now
            && closure.settings_revision == 3
            && closure.committed_at_entry == 900_000));
        assert_eq!(ledger, before);
    }
    let mut stopped = ledger.clone();
    stopped.closures.push(Closure {
        reason: ClosureReason::SpendStop,
        since: now,
        settings_revision: 3,
        committed_at_entry: 900_000,
    });
    let refusal = stopped.admit(&policy(), 3, 0, now).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::PlatformExhausted);
    assert!(refusal.closures.is_empty());
    assert!(stopped.closed_for_commitments());
}

#[test]
fn a_finished_period_is_metering_unavailable() {
    let mut ledger = observed_at(END - 10);
    let before = ledger.clone();
    let refusal = ledger.admit(&policy(), 1, 1, END).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::Unavailable);
    assert_eq!(
        reasons(&refusal.closures),
        vec![ClosureReason::MeteringUnavailable]
    );
    assert_eq!(ledger, before);
    let evaluation = ledger.evaluate(&policy(), 1, END).unwrap();
    assert_eq!(
        reasons(&evaluation.entered),
        vec![ClosureReason::MeteringUnavailable]
    );
    assert!(ledger.closed_for_commitments());
    let refusal = ledger.admit(&policy(), 1, 0, END).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::Unavailable);
    assert!(refusal.closures.is_empty());
}

#[test]
fn evaluation_reports_whether_anything_changed() {
    let now = START + 100;
    let mut quiet = observed_at(now);
    assert!(!quiet.evaluate(&policy(), 1, now).unwrap().changed());
    assert_eq!(quiet.evaluated_at, now);
    let mut ledger = observed_at(now);
    ledger.admit(&policy(), 1, 700_000, now).unwrap();
    ledger.reserved = 800_000;
    let first = ledger.evaluate(&policy(), 1, now).unwrap();
    assert!(first.changed());
    assert_eq!(reasons(&first.entered), vec![ClosureReason::SpendWarning]);
    let second = ledger.evaluate(&policy(), 1, now).unwrap();
    assert!(!second.changed());
    assert_eq!(second, Evaluation::default());
}

#[test]
fn closures_are_monotonic_within_a_revision_and_lift_only_under_a_newer_one() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    ledger.admit(&policy(), 1, 900_000, now).unwrap();
    let entered = ledger.evaluate(&policy(), 1, now).unwrap();
    assert_eq!(reasons(&entered.entered), vec![ClosureReason::SpendStop]);
    assert_eq!(
        reasons(&ledger.closures),
        vec![ClosureReason::SpendWarning, ClosureReason::SpendStop]
    );
    ledger.release(900_000).unwrap();
    assert!(!ledger.evaluate(&policy(), 1, now).unwrap().changed());
    assert!(ledger.closed_for_commitments());
    let mut raised = policy();
    raised.period_cap = 2_000_000;
    ledger.reserved = 1_700_000;
    let partial = ledger.evaluate(&raised, 2, now).unwrap();
    assert_eq!(partial.lifted, vec![ClosureReason::SpendStop]);
    assert_eq!(reasons(&ledger.closures), vec![ClosureReason::SpendWarning]);
    ledger.reserved = 0;
    let lifted = ledger.evaluate(&raised, 2, now).unwrap();
    assert_eq!(lifted.lifted, vec![ClosureReason::SpendWarning]);
    assert!(ledger.closures.is_empty());
    assert_eq!(ledger.settings_revision, 2);
}

#[test]
fn metering_unavailable_lifts_on_a_fresh_observation_without_a_new_revision() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    let stale = now + 3_601;
    let entered = ledger.evaluate(&policy(), 4, stale).unwrap();
    assert_eq!(
        reasons(&entered.entered),
        vec![ClosureReason::MeteringUnavailable]
    );
    assert!(!ledger.evaluate(&policy(), 4, stale).unwrap().changed());
    ledger.record_cloud(observation(5, stale)).unwrap();
    let lifted = ledger.evaluate(&policy(), 4, stale).unwrap();
    assert_eq!(lifted.lifted, vec![ClosureReason::MeteringUnavailable]);
    assert!(lifted.entered.is_empty());
    assert!(!ledger.closed_for_commitments());
}

#[test]
fn period_changes_fail_closed_under_liability() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    ledger.admit(&policy(), 1, 10, now).unwrap();
    let mut moved = policy();
    moved.period_end = "2023-12-24T22:13:20Z".into();
    let refusal = ledger.admit(&moved, 2, 10, now).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::Unavailable);
    assert_eq!(
        reasons(&refusal.closures),
        vec![ClosureReason::MeteringUnavailable]
    );
    assert_eq!(ledger.reserved, 10);
    let mut empty = observed_at(now);
    assert_eq!(empty.admit(&moved, 2, 10, now), Ok(vec![]));
    assert_eq!(empty.period_end, END + 864_000);
}

#[test]
fn settle_release_and_retain_keep_spent_plus_reserved_consistent() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    ledger.admit(&policy(), 1, 100, now).unwrap();
    ledger.retain(20).unwrap();
    assert_eq!(ledger.reserved, 120);
    ledger.settle(120, 150).unwrap();
    assert_eq!((ledger.spent, ledger.reserved), (150, 0));
    ledger.admit(&policy(), 1, 50, now).unwrap();
    ledger.release(50).unwrap();
    assert_eq!((ledger.spent, ledger.reserved), (150, 0));
    assert_eq!(ledger.committed(&policy(), now), Ok(150));
    let before = ledger.clone();
    assert_eq!(ledger.release(1), Err(BudgetError::Unavailable));
    assert_eq!(ledger.settle(1, 1), Err(BudgetError::Unavailable));
    assert_eq!(ledger.retain(u64::MAX), Err(BudgetError::Unavailable));
    assert_eq!(ledger, before);
}

#[test]
fn missing_cloud_policy_is_unavailable_not_zero() {
    let now = START + 100;
    let mut unconfigured = policy();
    unconfigured.cloud = None;
    unconfigured.validate().unwrap();
    let mut ledger = observed_at(now);
    assert_eq!(
        ledger.cloud_committed(&unconfigured, now),
        Err(BudgetError::Unavailable)
    );
    let refusal = ledger.admit(&unconfigured, 1, 0, now).unwrap_err();
    assert_eq!(refusal.reason, BudgetError::Unavailable);
    assert_eq!(
        reasons(&refusal.closures),
        vec![ClosureReason::MeteringUnavailable]
    );
    let unobserved = PlatformPeriodLedger::open(&policy(), 1).unwrap();
    assert_eq!(
        unobserved.committed(&policy(), now),
        Err(BudgetError::Unavailable)
    );
    let entered = ledger.evaluate(&unconfigured, 1, now).unwrap();
    assert_eq!(
        reasons(&entered.entered),
        vec![ClosureReason::MeteringUnavailable]
    );
}

#[test]
fn the_period_row_sums_valued_amounts_and_pins_no_valuation() {
    let now = START + 100;
    let mut ledger = observed_at(now);
    // Two owners priced differently: each admits an already valued amount.
    assert_eq!(ledger.admit(&policy(), 1, 300, now), Ok(vec![]));
    assert_eq!(ledger.admit(&policy(), 1, 700, now), Ok(vec![]));
    assert_eq!(ledger.reserved, 1_000);
    let persisted = serde_json::to_value(&ledger).unwrap();
    assert!(persisted.get("token_units").is_none());
    let mut legacy = persisted;
    legacy["token_units"] = serde_json::json!([1, 1]);
    assert!(serde_json::from_value::<PlatformPeriodLedger>(legacy).is_err());
}
