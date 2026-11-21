use super::FeatureFlags;
const KEY: &str = "CHRONO_PENDING_HOME_RECONCILIATION_ENABLED";

#[test]
fn pending_home_recovery_is_independent_strict_and_default_off() {
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .pending_home_reconciliation
    );
    for enabled in [false, true] {
        let flags =
            FeatureFlags::from_lookup(&mut |name| Ok((name == KEY).then(|| enabled.to_string())))
                .unwrap();
        assert_eq!(flags.pending_home_reconciliation, enabled);
        assert!(!flags.admission_ceilings_writes_enabled);
    }
    let flags = FeatureFlags::from_lookup(&mut |name| {
        Ok((name == "CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED").then(|| "true".into()))
    })
    .unwrap();
    assert!(!flags.pending_home_reconciliation);
    for value in ["", "1", "TRUE", " true", "false "] {
        assert!(
            FeatureFlags::from_lookup(&mut |name| Ok((name == KEY).then(|| value.into()))).is_err()
        );
    }
    assert!(FeatureFlags::from_lookup(&mut |name| if name == KEY {
        Err("not unicode".into())
    } else {
        Ok(None)
    })
    .is_err());
}
