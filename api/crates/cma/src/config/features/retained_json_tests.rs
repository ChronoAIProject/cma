use super::*;
#[test]
fn json_retention_requires_its_own_strict_opt_in() {
    let key = "CHRONO_RUNTIME_JSON_RETENTION_BOUNDS_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.retained_json, expected);
        assert!(!flags.bounded_raw_output && !flags.output_json_budget);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then(|| value.to_owned()))
        })
        .is_err());
    }
}
