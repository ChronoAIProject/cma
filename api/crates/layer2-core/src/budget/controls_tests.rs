use super::*;

fn policy(zone: &str) -> BudgetPolicy {
    let unit = u64::from(u8::MAX);
    BudgetPolicy {
        owner_day_limit: unit * unit,
        turn_limit: unit,
        max_calls_per_turn: unit,
        day_timezone: zone.into(),
        input_token_units: 1,
        output_token_units: 1,
        max_unobserved_units: 1,
        usage_timeout_seconds: 60,
    }
}

fn instant(value: &str) -> u64 {
    crate::seconds_from_rfc3339(value).unwrap()
}

#[test]
fn next_reset_matches_the_accounting_window_boundary() {
    for (zone, now, reset) in [
        ("UTC", "2026-11-30T23:59:59Z", "2026-12-01T00:00:00Z"),
        ("UTC", "2026-12-01T00:00:00Z", "2026-12-02T00:00:00Z"),
        (
            "Asia/Singapore",
            "2026-11-30T15:59:59Z",
            "2026-11-30T16:00:00Z",
        ),
        (
            "America/New_York",
            "2026-03-08T05:00:00Z",
            "2026-03-09T04:00:00Z",
        ),
        (
            "Pacific/Apia",
            "2011-12-30T09:59:59Z",
            "2011-12-30T10:00:00Z",
        ),
    ] {
        let budget = policy(zone);
        let now = instant(now);
        let reset = instant(reset);
        assert_eq!(budget.next_reset_at(now), Ok(reset), "{zone}");
        assert_eq!(budget.day(reset - 1), budget.day(now), "{zone}");
        assert!(
            budget.day(reset).unwrap() > budget.day(now).unwrap(),
            "{zone}"
        );
    }
}

#[test]
fn local_midnight_and_dst_boundaries_reset_spend_without_freeing_holds() {
    for (zone, before, after) in [
        (
            "Asia/Singapore",
            "2026-11-10T15:59:59Z",
            "2026-11-10T16:00:00Z",
        ),
        (
            "America/New_York",
            "2026-03-09T03:59:59Z",
            "2026-03-09T04:00:00Z",
        ),
        (
            "America/New_York",
            "2026-11-02T04:59:59Z",
            "2026-11-02T05:00:00Z",
        ),
    ] {
        let policy = policy(zone);
        let mut owner = OwnerBudget::default();
        owner.reserve(&policy, instant(before)).unwrap();
        owner.spent = 1;
        let previous_day = owner.day;
        owner.advance(&policy, instant(after)).unwrap();
        assert_eq!(owner.day, previous_day + 1);
        assert_eq!(owner.spent, 0);
        assert_eq!(owner.reserved, policy.turn_limit);
    }
}

#[test]
fn timezone_edits_cannot_reinterpret_existing_liability() {
    let policy = policy("UTC");
    let mut owner = OwnerBudget::default();
    let now = instant("2026-11-11T00:00:00Z");
    owner.reserve(&policy, now).unwrap();
    let before = owner.clone();
    let mut edited = policy;
    edited.day_timezone = "Asia/Singapore".into();
    assert_eq!(owner.advance(&edited, now), Err(BudgetError::Unavailable));
    assert_eq!(owner, before);
    owner.reserved = 0;
    owner.advance(&edited, now).unwrap();
    assert_eq!(owner.day_timezone.as_deref(), Some("Asia/Singapore"));
}

#[test]
fn disabled_is_the_only_implicit_spend_control_setting() {
    let settings = BudgetSettings::default();
    assert!(!settings.enabled);
    assert!(settings.platform.is_none());
    assert!(settings.default_policy.is_none());
    assert!(settings.owners.is_empty());
    let mut invalid = settings;
    invalid.enabled = true;
    assert_eq!(invalid.validate(), Err(BudgetError::InvalidPolicy));
    assert_eq!(
        policy("invalid/timezone").validate(),
        Err(BudgetError::InvalidPolicy)
    );
}
