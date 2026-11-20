use super::FeatureFlags;

#[test]
fn visitor_defaults_require_explicit_strict_boolean() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == "CHRONO_VISITOR_TWO_SANDBOX_DEFAULTS_ENABLED" {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.visitor_two_sandbox_defaults, expected);
        assert_eq!(
            crate::store::settings_defaults::InitialSettings::new(
                flags.visitor_two_sandbox_defaults
            )
            .platform()
            .limits
            .visitor_max_sandboxes,
            if expected { 2 } else { 1 },
        );
    }
    for invalid in ["", "1", "TRUE", "true ", " false"] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_VISITOR_TWO_SANDBOX_DEFAULTS_ENABLED")
            .then(|| invalid.into())))
        .is_err());
    }
}
