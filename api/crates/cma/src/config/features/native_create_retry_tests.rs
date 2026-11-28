use super::FeatureFlags;

#[test]
fn native_create_retry_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_SANDBOX_NATIVE_CREATE_RETRY_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.native_create_retry_enabled, expected);
        assert!(!flags.work_retry_policy_enabled);
    }
    for value in ["", "1", "TRUE", " true ", "False"] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .is_err()
        );
    }
}

#[test]
fn runtime_update_reopen_output_refusal_is_strict_and_default_off() {
    const KEY: &str = "CHRONO_RUNTIME_UPDATE_REOPEN_OUTPUT_REFUSAL_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then_some(value).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.reopen_output_refusal_enabled, expected);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".to_owned()))).is_err()
    );
}
