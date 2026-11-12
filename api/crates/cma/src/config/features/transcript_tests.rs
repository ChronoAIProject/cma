use super::*;
#[test]
fn transcript_reads_require_an_independent_strict_opt_in() {
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_transcript_reads, enabled);
        assert!(!flags.bounded_response_reads && !flags.bounded_title_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED")
            .then(|| value.into())))
        .is_err());
    }
}
