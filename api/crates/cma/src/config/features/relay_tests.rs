use super::*;

#[test]
fn agent_file_write_handoff_is_default_off_and_strict() {
    const KEY: &str = "CHRONO_AGENT_FILE_WRITE_HANDOFF_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then_some(value).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.agent_file_write_handoff, enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".into()))).is_err());
}

#[test]
fn agent_settings_owner_relay_is_default_off_and_strict() {
    const KEY: &str = "CHRONO_AGENT_SETTINGS_OWNER_RELAY_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then_some(value).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.agent_settings_owner_relay, enabled);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".into()))).is_err());
}

#[test]
fn bounded_update_relay_requires_a_separate_strict_boolean() {
    const KEY: &str = "CHRONO_RUNTIME_UPDATE_RELAY_BOUNDED_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_update_relay, enabled);
        assert!(!flags.bounded_close_retries);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}
