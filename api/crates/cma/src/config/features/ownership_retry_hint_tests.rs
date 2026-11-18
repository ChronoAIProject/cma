use super::FeatureFlags;

#[test]
fn ownership_retry_hint_requires_explicit_strict_boolean() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == "CHRONO_AGENT_OWNERSHIP_RETRY_HINT_ENABLED" {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.ownership_retry_hints_enabled, expected);
    }
    for invalid in ["", "1", "TRUE", "true ", " false"] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_OWNERSHIP_RETRY_HINT_ENABLED")
            .then(|| invalid.into())))
        .is_err());
    }
}
