use super::*;

#[test]
fn recovery_process_switch_defaults_off_and_rejects_ambiguous_values() {
    let key = "CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED";
    let base = BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "sandbox-test".into()),
        ("CHRONO_RUNTIME_HOME".into(), "/home/cma".into()),
    ]);
    assert!(
        !RuntimeConfig::from_values(base.clone())
            .unwrap()
            .capacity_recovery_enabled
    );
    for (value, expected) in [("0", false), ("false", false), ("1", true), ("true", true)] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        let config = RuntimeConfig::from_values(env).unwrap();
        assert_eq!(config.capacity_recovery_enabled, expected);
        assert!(!config.install_env.contains_key(key));
    }
    for value in ["", "TRUE", "yes", " true", "2"] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        assert!(RuntimeConfig::from_values(env).is_err());
    }
}

#[test]
fn legacy_journal_recovery_requires_its_own_explicit_switch() {
    let key = "CHRONO_RUNTIME_LEGACY_JOURNAL_RECOVERY_ENABLED";
    let base = BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "sandbox-test".into()),
        ("CHRONO_RUNTIME_HOME".into(), "/home/cma".into()),
        (
            "CHRONO_RUNTIME_CAPACITY_RECOVERY_ENABLED".into(),
            "true".into(),
        ),
    ]);
    assert!(
        !RuntimeConfig::from_values(base.clone())
            .unwrap()
            .legacy_journal_recovery_enabled
    );
    for (value, expected) in [("0", false), ("false", false), ("1", true), ("true", true)] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        let config = RuntimeConfig::from_values(env).unwrap();
        assert_eq!(config.legacy_journal_recovery_enabled, expected);
        assert!(!config.install_env.contains_key(key));
    }
    for value in ["", "TRUE", "yes", " true", "2"] {
        let mut env = base.clone();
        env.insert(key.into(), value.into());
        assert!(RuntimeConfig::from_values(env).is_err());
    }
}
