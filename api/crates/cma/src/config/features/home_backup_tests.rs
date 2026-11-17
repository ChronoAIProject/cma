use super::{home_backup::HomeBackupConfig, FeatureFlags};

fn settings(values: &[(&str, &str)], enabled: bool) -> Result<HomeBackupConfig, String> {
    HomeBackupConfig::from_lookup(
        &mut |key| {
            Ok(values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, v)| v.to_string()))
        },
        enabled,
    )
}

#[test]
fn capture_is_disabled_by_default_and_class_is_required_for_enablement() {
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .home_backup_enabled
    );
    let defaults = settings(&[], false).unwrap();
    assert!(defaults.snapshot_class.is_none());
    assert_eq!(defaults.interval_seconds, 1800);
    assert_eq!(defaults.max_in_flight, 2);
    assert!(settings(&[], true).is_err());
    let configured = settings(
        &[("CHRONO_HOME_BACKUP_SNAPSHOT_CLASS", "retained-class")],
        true,
    )
    .unwrap();
    assert_eq!(configured.snapshot_class.as_deref(), Some("retained-class"));
    for value in ["TRUE", "1", "", " true"] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok(
            (key == "CHRONO_HOME_BACKUP_ENABLED").then(|| value.into())
        ))
        .is_err());
    }
}

#[test]
fn bounds_and_class_are_strict_even_with_writer_disabled() {
    for (key, value) in [
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", "0"),
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", "1801"),
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", " 30"),
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", "+30"),
        (
            "CHRONO_HOME_BACKUP_INTERVAL_SECONDS",
            "18446744073709551616",
        ),
        ("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", "0"),
        ("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", "-1"),
        ("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", "65536"),
        ("CHRONO_HOME_BACKUP_SNAPSHOT_CLASS", "bad/class"),
        ("CHRONO_HOME_BACKUP_SNAPSHOT_CLASS", "Upper"),
        ("CHRONO_HOME_BACKUP_SNAPSHOT_CLASS", "-class"),
    ] {
        assert!(settings(&[(key, value)], false).unwrap_err().contains(key));
    }
    for (key, value) in [
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", "1"),
        ("CHRONO_HOME_BACKUP_INTERVAL_SECONDS", "1800"),
        ("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", "1"),
        ("CHRONO_HOME_BACKUP_MAX_IN_FLIGHT", "65535"),
    ] {
        assert!(settings(&[(key, value)], false).is_ok());
    }
    assert_eq!(
        HomeBackupConfig::from_lookup(&mut |_| Err("lookup".into()), false).unwrap_err(),
        "lookup"
    );
}
