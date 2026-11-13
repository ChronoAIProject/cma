use super::*;

#[test]
fn approval_reader_rollout_requires_an_explicit_boolean() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_approval_reads, expected);
        assert!(!flags.bounded_title_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_APPROVAL_BOUNDED_READS_ENABLED")
            .then(|| value.to_owned())))
        .is_err());
    }
}
