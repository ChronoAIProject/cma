use super::FeatureFlags;

#[test]
fn admission_control_write_gate_is_independent_default_closed_and_strict() {
    const KEY: &str = "CHRONO_ADMISSION_CONTROL_WRITES_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.admission_control_writes_enabled, expected);
        assert!(!flags.model_budget_writes_enabled);
        assert!(!flags.platform_spend_writes_enabled);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |name| {
            Ok((name == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}
