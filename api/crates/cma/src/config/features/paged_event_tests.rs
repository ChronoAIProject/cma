use super::*;
#[test]
fn paged_events_are_default_off_and_require_their_own_strict_boolean() {
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.paged_response_events, enabled);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")
            .then(|| value.into())))
        .is_err());
    }
}
