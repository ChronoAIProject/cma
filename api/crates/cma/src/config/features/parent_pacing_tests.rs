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
