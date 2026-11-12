use super::*;

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
