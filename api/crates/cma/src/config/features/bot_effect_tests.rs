use super::FeatureFlags;

#[test]
fn bot_effect_format_rollout_requires_strict_independent_opt_in() {
    const KEY: &str = "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == KEY {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.bot_effect_reservations, expected);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .is_err()
        );
    }
}
