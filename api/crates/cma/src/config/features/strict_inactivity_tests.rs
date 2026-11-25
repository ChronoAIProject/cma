use super::FeatureFlags;

#[test]
fn strict_inactivity_session_evidence_defaults_off() {
    const KEY: &str = "CHRONO_STRICT_INACTIVITY_SESSION_EVIDENCE_ENABLED";
    for (configured, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(configured.filter(|_| key == KEY).map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.strict_inactivity_session_evidence, expected);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| { Ok((key == KEY).then(|| "yes".into())) }).is_err()
    );
}
