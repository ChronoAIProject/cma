use super::FeatureFlags;

#[test]
fn native_close_agent_lane_requires_explicit_opt_in() {
    const KEY: &str = "CHRONO_NATIVE_CLOSE_AGENT_LANE_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == KEY {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.native_close_agent_lane, enabled);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| { Ok((key == KEY).then(|| "yes".to_owned())) })
            .is_err()
    );
}
