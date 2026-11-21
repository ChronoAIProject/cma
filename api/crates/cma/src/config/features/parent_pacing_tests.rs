use super::*;

#[test]
fn parent_readiness_pacing_is_independent_and_default_off() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.parent_not_ready_pacing_enabled, expected);
        assert!(!flags.bounded_close_retries);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED").then(|| value.into()))
        })
        .is_err());
    }
}

#[test]
fn runtime_startup_sibling_hold_is_independent_strict_and_default_off() {
    const KEY: &str = "CHRONO_AGENT_STARTUP_SIBLING_HOLD_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.startup_sibling_hold_enabled, expected);
        assert!(!flags.parent_not_ready_pacing_enabled);
    }
    for value in ["", "1", "TRUE", " true "] {
        let error =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .unwrap_err();
        assert_eq!(error, format!("{KEY} must be true or false"));
    }
}
