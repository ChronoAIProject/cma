use super::*;

#[test]
fn startup_health_serving_requires_a_strict_opt_in() {
    let key = "CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok((name == key).then_some(value).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.am_startup_health_serving, expected);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |name| Ok((name == key).then(|| "yes".into()))).is_err()
    );
}

#[test]
fn empty_warm_pool_startup_requires_early_http_and_explicit_opt_in() {
    for (early, empty, expected) in [
        (false, None, Some(false)),
        (true, None, Some(false)),
        (true, Some("false"), Some(false)),
        (true, Some("true"), Some(true)),
        (false, Some("true"), None),
        (true, Some("yes"), None),
    ] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok(match name {
                "CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED" => Some(early.to_string()),
                "CHRONO_AM_EMPTY_WARM_POOL_STARTUP_ENABLED" => empty.map(str::to_owned),
                _ => None,
            })
        });
        assert_eq!(
            flags.ok().map(|flags| flags.am_empty_warm_pool_startup),
            expected
        );
    }
}

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
