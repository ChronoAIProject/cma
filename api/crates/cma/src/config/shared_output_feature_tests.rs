use super::*;

#[test]
fn shared_runtime_output_requires_its_own_strict_opt_in() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.shared_runtime_output, expected);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED").then(|| value.into()))
        })
        .is_err());
    }
}
