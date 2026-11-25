use super::FeatureFlags;

#[test]
fn native_retirement_finality_defaults_off_and_requires_explicit_selection() {
    const KEY: &str = "CHRONO_NATIVE_RETIREMENT_FINALITY_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.native_retirement_finality_enabled, expected);
        assert!(!flags.runtime_loss_bounded_proof_enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "yes".into()))).is_err());
}

#[test]
fn suspended_reopen_admission_is_independent_and_defaults_off() {
    const KEY: &str = "CHRONO_RETAINED_SUSPENDED_REOPEN_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.retained_suspended_reopen_enabled, expected);
        assert!(!flags.retained_reopen_enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "yes".into()))).is_err());
}

#[test]
fn runtime_loss_proof_requires_an_independent_strict_default_off_flag() {
    const KEY: &str = "CHRONO_RUNTIME_LOSS_BOUNDED_PROOF_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.runtime_loss_bounded_proof_enabled, enabled);
        assert!(!flags.segment_adoption_enabled);
        assert!(!flags.capacity_recovery_enabled);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .is_err()
        );
    }
    assert!(FeatureFlags::from_lookup(&mut |key| if key == KEY {
        Err("unavailable".into())
    } else {
        Ok(None)
    })
    .is_err());
}

#[test]
fn repeat_reopen_admission_is_independent_and_defaults_off() {
    const KEY: &str = "CHRONO_RETAINED_REPEAT_REOPEN_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.retained_repeat_reopen_enabled, expected);
        assert!(!flags.retained_reopen_enabled);
        assert!(!flags.retained_suspended_reopen_enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "yes".into()))).is_err());
}

#[test]
fn native_retirement_capture_is_independent_and_defaults_off() {
    const KEY: &str = "CHRONO_NATIVE_RETIREMENT_CAPTURE_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.native_retirement_capture_enabled, expected);
        assert!(!flags.native_retirement_finality_enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "yes".into()))).is_err());
}
