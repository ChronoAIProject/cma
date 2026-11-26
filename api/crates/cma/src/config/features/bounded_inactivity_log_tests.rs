use super::FeatureFlags;

#[test]
fn legacy_inactivity_log_budget_defaults_off() {
    const KEY: &str = "CHRONO_BOUNDED_INACTIVITY_LEGACY_LOG_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(value.filter(|_| key == KEY).map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.bounded_inactivity_legacy_log, enabled);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "invalid".into()))).is_err()
    );
}
