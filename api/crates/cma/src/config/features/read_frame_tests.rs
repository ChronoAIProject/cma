use super::*;

#[test]
fn read_frame_ownership_requires_an_independent_strict_boolean() {
    const KEY: &str = "CHRONO_READ_FRAME_BUDGET_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.retain_read_frames, enabled);
        assert!(!flags.bounded_response_reads && !flags.bounded_title_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}
