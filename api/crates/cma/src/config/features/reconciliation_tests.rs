use super::*;

#[test]
fn sandbox_reserved_lane_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_SANDBOX_RECONCILIATION_RESERVED_LANE_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.sandbox_reconciliation_reserved_lane, expected);
        assert!(!flags.event_driven_reconciliation);
        assert!(!flags.command_reconcile_pacing);
    }
    for value in ["", "1", "TRUE", " true ", "false "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
    assert!(FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("lookup unavailable".into())
        } else {
            Ok(None)
        }
    })
    .is_err());
}

#[test]
fn event_driven_reconciliation_requires_an_explicit_boolean_opt_in() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(
                if key == "CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED" {
                    value.map(str::to_owned)
                } else {
                    None
                },
            )
        })
        .unwrap();
        assert_eq!(flags.event_driven_reconciliation, expected);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_RECONCILIATION_EVENT_DRIVEN_ENABLED")
                .then(|| value.to_owned()))
        })
        .is_err());
    }
}

#[test]
fn command_reconciliation_pacing_is_independent_strict_and_default_off() {
    const KEY: &str = "CHRONO_COMMAND_RECONCILE_PACING_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.command_reconcile_pacing, expected);
        assert!(!flags.event_driven_reconciliation);
    }
    for value in ["", "1", "TRUE", " true ", "false "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| { Ok((key == KEY).then(|| value.into())) })
                .is_err()
        );
    }
    assert!(FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("lookup unavailable".into())
        } else {
            Ok(None)
        }
    })
    .is_err());
}
