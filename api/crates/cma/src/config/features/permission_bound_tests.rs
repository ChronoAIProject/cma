use super::*;

#[test]
fn permission_state_requires_its_own_strict_opt_in() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_permission_state, expected);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_RUNTIME_PERMISSION_BOUNDS_ENABLED").then(|| value.into()))
        })
        .is_err());
    }
}
