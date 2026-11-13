use super::*;

#[test]
fn live_buffer_budget_requires_its_own_strict_boolean() {
    const KEY: &str = "CHRONO_AGENT_LIVE_BUFFER_BUDGET_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_live_fragments, enabled);
        assert!(!flags.bounded_close_retries);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}
