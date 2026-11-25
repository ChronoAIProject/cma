use super::FeatureFlags;

const KEY: &str = "CHRONO_HOME_BACKUP_EXPIRY_ENABLED";

#[test]
fn expiry_is_default_off_and_accepts_only_explicit_boolean_values() {
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .home_backup_expiry_enabled
    );
    for (value, expected) in [("false", false), ("true", true)] {
        assert_eq!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into())))
                .unwrap()
                .home_backup_expiry_enabled,
            expected
        );
    }
    for value in ["", "TRUE", "1", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into())))
                .unwrap_err()
                .contains(KEY)
        );
    }
}

#[test]
fn expiry_preserves_configuration_lookup_failures() {
    let error = FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("expiry lookup unavailable".into())
        } else {
            Ok(None)
        }
    })
    .unwrap_err();
    assert_eq!(error, "expiry lookup unavailable");
}
