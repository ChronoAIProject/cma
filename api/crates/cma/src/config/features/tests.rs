use super::*;

#[test]
fn document_quotas_require_an_independent_strict_opt_in() {
    let key = "CHRONO_DOCUMENT_QUOTAS_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.document_quotas, enabled);
        assert!(!flags.document_revocation_reserve);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |name| Ok((name == key).then(|| value.to_owned())))
                .is_err()
        );
    }
}

#[test]
fn bounded_title_reads_require_an_explicit_boolean_opt_in() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok(if key == "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED" {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.bounded_title_reads, expected);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_TITLE_BOUNDED_READS_ENABLED").then(|| value.into()))
        })
        .is_err());
    }
}
