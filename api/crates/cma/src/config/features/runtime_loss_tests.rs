use super::FeatureFlags;

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
