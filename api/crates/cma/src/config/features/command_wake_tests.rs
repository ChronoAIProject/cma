use super::*;

#[test]
fn command_discovery_wake_is_independent_strict_and_default_off() {
    const KEY: &str = "CHRONO_COMMAND_DISCOVERY_WAKE_ENABLED";
    for (raw, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == KEY).then_some(raw).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.command_discovery_wake, expected);
        assert!(!flags.command_reconcile_pacing);
    }
    for raw in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |name| {
            Ok((name == KEY).then(|| raw.to_owned()))
        })
        .is_err());
    }
}
