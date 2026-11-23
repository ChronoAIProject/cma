use super::FeatureFlags;

#[test]
fn stale_turn_discovery_requires_explicit_opt_in() {
    const KEY: &str = "CHRONO_STALE_TURN_RECONCILIATION_ENABLED";
    let parse = |value: Option<&str>| {
        FeatureFlags::from_lookup(&mut |key| Ok(value.filter(|_| key == KEY).map(str::to_owned)))
    };
    assert!(!parse(None).unwrap().stale_turn_reconciliation);
    assert!(!parse(Some("false")).unwrap().stale_turn_reconciliation);
    assert!(parse(Some("true")).unwrap().stale_turn_reconciliation);
    assert!(parse(Some("1")).unwrap_err().contains(KEY));
}
