use super::*;

#[test]
fn work_retry_policy_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_WORK_RETRY_POLICY_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.work_retry_policy_enabled, expected);
        assert!(!flags.retry_single_marker_enabled);
        assert!(!flags.parent_not_ready_pacing_enabled);
    }
    for value in ["", "1", "TRUE", " true ", "False"] {
        let error =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .unwrap_err();
        assert!(error.contains(KEY));
    }
}

#[test]
fn cleanup_continuation_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_AGENT_CLEANUP_CONTINUATION_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.cleanup_continuation_enabled, expected);
        assert!(!flags.work_retry_policy_enabled);
        assert!(!flags.bounded_close_retries);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .unwrap_err()
                .contains(KEY)
        );
    }
}
