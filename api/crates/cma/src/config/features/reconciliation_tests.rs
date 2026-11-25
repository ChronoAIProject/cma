use super::*;

#[test]
fn startup_native_home_hold_is_strict_and_default_off() {
    const KEY: &str = "CHRONO_STARTUP_NATIVE_HOME_HOLD_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.startup_native_home_hold_enabled, expected);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".into()))).is_err());
}

#[test]
fn binding_diagnostics_are_independent_strict_and_default_off() {
    const KEY: &str = "CHRONO_BINDING_DIAGNOSTICS_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.binding_diagnostics, enabled);
        assert!(!flags.sandbox_binding_hold_metrics);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
        );
    }
}

#[test]
fn sandbox_profile_readability_and_binding_hold_metrics_are_independent_opt_ins() {
    const READABILITY: &str = "CHRONO_SANDBOX_PROFILE_READABILITY_ENABLED";
    const HOLD_METRICS: &str = "CHRONO_SANDBOX_BINDING_HOLD_METRICS_ENABLED";
    for selected in [None, Some(READABILITY), Some(HOLD_METRICS)] {
        let flags =
            FeatureFlags::from_lookup(
                &mut |key| Ok((Some(key) == selected).then(|| "true".into())),
            )
            .unwrap();
        assert_eq!(
            flags.sandbox_profile_readability,
            selected == Some(READABILITY)
        );
        assert_eq!(
            flags.sandbox_binding_hold_metrics,
            selected == Some(HOLD_METRICS)
        );
    }
    for key in [READABILITY, HOLD_METRICS] {
        assert!(FeatureFlags::from_lookup(&mut |lookup| {
            Ok((lookup == key).then(|| "1".into()))
        })
        .is_err());
    }
}

#[test]
fn batch_ownership_renewal_is_strict_and_default_off() {
    const KEY: &str = "CHRONO_AM_BATCH_OWNERSHIP_RENEWAL";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == KEY {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.batch_ownership_renewal, enabled);
        assert!(flags.ownership_cleanup_pacing);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".into()))).is_err());
}

#[test]
fn ownership_cleanup_pacing_is_independent_strict_and_default_on() {
    const KEY: &str = "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED";
    for (value, expected) in [(None, true), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.ownership_cleanup_pacing, expected);
        assert!(flags.command_reconcile_pacing);
        assert!(!flags.work_retry_policy_enabled);
    }
    for value in ["", "1", "TRUE", " true ", "false "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
        );
    }
    assert!(FeatureFlags::from_lookup(&mut |key| if key == KEY {
        Err("lookup unavailable".into())
    } else {
        Ok(None)
    })
    .is_err());
}

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
        assert!(flags.command_reconcile_pacing);
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
fn agent_reconciliation_continuation_is_independent_and_default_off() {
    const KEY: &str = "CHRONO_AGENT_RECONCILIATION_RESUME_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == KEY {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.resume_agent_reconciliation, expected);
        assert!(!flags.event_driven_reconciliation);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".to_owned()))).is_err()
    );
}

#[test]
fn command_cancel_wake_is_strict_and_default_off() {
    const KEY: &str = "CHRONO_COMMAND_CANCEL_WAKE_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.command_cancel_wake, expected);
        assert!(flags.command_reconcile_pacing);
    }
    assert!(FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| "1".into()))).is_err());
}

#[test]
fn command_reconciliation_pacing_is_independent_strict_and_default_on() {
    const KEY: &str = "CHRONO_COMMAND_RECONCILE_PACING_ENABLED";
    for (value, expected) in [(None, true), (Some("false"), false), (Some("true"), true)] {
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

#[test]
fn agent_validation_reuse_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_AGENT_RECONCILIATION_VALIDATION_REUSE_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.agent_connection_observations, expected);
        assert!(!flags.event_driven_reconciliation);
        assert!(flags.command_reconcile_pacing);
    }
    for value in ["", "1", "TRUE", " true ", "false "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
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

#[test]
fn empty_discovery_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_EMPTY_DISCOVERY_COALESCING_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.empty_discovery, expected);
        assert!(!flags.agent_connection_observations);
        assert!(flags.command_reconcile_pacing);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
        );
    }
}

#[test]
fn receiver_housekeeping_containment_is_strict_independent_and_default_off() {
    const KEY: &str = "CHRONO_RECEIVER_HOUSEKEEPING_CONTAINMENT_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.receiver_housekeeping_containment, expected);
        assert!(flags.ownership_cleanup_pacing);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
        );
    }
}
