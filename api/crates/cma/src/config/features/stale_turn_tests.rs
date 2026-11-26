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

#[test]
fn stale_turn_terminal_requires_its_own_explicit_opt_in() {
    const DISCOVERY: &str = "CHRONO_STALE_TURN_RECONCILIATION_ENABLED";
    const TERMINAL: &str = "CHRONO_STALE_TURN_TERMINAL_ENABLED";
    let parse = |enabled, value: &str| {
        FeatureFlags::from_lookup(&mut |key| Ok((key == enabled).then(|| value.to_owned())))
    };
    assert!(!parse(DISCOVERY, "true").unwrap().stale_turn_terminal);
    let flags = parse(TERMINAL, "true").unwrap();
    assert!(flags.stale_turn_terminal);
    assert!(!flags.stale_turn_reconciliation);
    assert!(!parse(TERMINAL, "false").unwrap().stale_turn_terminal);
    assert!(parse(TERMINAL, "1").unwrap_err().contains(TERMINAL));
}
