
use super::*;

#[test]
fn raw_output_bounds_require_their_own_strict_opt_in() {
    let key = "CHRONO_RUNTIME_RAW_OUTPUT_BOUNDS_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_raw_output, expected);
        assert!(!flags.retained_reopen_enabled && !flags.bounded_approval_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then(|| value.to_owned()))
        })
        .is_err());
    }
}
