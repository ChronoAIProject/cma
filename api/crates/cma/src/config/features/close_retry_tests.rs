use super::*;

#[test]
fn close_retry_optimization_requires_its_own_strict_boolean() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED" {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.bounded_close_retries, expected);
        assert!(!flags.bounded_title_reads && !flags.bounded_approval_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_CLOSE_RETRY_BOUNDED_ENABLED").then(|| value.into()))
        })
        .is_err());
    }
}
