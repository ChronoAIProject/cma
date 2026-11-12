use super::*;

#[test]
fn bounded_update_relay_requires_a_separate_strict_boolean() {
    const KEY: &str = "CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_update_relay, enabled);
        assert!(!flags.bounded_close_retries);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}
