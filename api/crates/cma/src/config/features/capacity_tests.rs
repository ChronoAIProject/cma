use super::FeatureFlags;

#[test]
fn capacity_recovery_admission_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.capacity_recovery_enabled, expected);
        assert!(!flags.parent_not_ready_pacing_enabled);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| { Ok((key == KEY).then(|| value.into())) })
                .is_err()
        );
    }
}
