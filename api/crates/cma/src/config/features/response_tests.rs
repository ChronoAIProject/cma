use super::*;

#[test]
fn bounded_response_reads_require_their_own_strict_opt_in() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_response_reads, expected);
        assert!(!flags.bounded_approval_reads && !flags.bounded_title_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_RESPONSE_BOUNDED_READS_ENABLED")
            .then(|| value.into())))
        .is_err());
    }
}
