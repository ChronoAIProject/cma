use super::*;

#[test]
fn retry_single_marker_is_strict_independent_and_default_off() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.retry_single_marker_enabled, expected);
        assert!(!flags.bounded_close_retries);
        assert!(!flags.parent_not_ready_pacing_enabled);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_RETRY_SINGLE_MARKER_ENABLED")
            .then(|| value.into())))
        .is_err());
    }
}
