use super::FeatureFlags;

#[test]
fn document_object_deletion_requires_an_independent_strict_opt_in() {
    const KEY: &str = "CHRONO_DOCUMENT_OBJECT_DELETION_ENABLED";
    let defaults = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
    assert!(!defaults.document_object_deletion);
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags != defaults, enabled);
        assert_eq!(flags.document_object_deletion, enabled);
        assert!(!flags.document_quotas);
        assert!(!flags.document_revocation_reserve);
    }
    for value in ["", "1", "TRUE", " true ", "FALSE", "0"] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
}

#[test]
fn document_source_commits_default_off_and_require_a_strict_opt_in() {
    const KEY: &str = "CHRONO_DOCUMENT_SOURCE_COMMITS_ENABLED";
    let defaults = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
    assert!(!defaults.document_source_commits);
    for (value, enabled) in [("false", false), ("true", true)] {
        let flags =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())))
                .unwrap();
        assert_eq!(flags.document_source_commits, enabled);
        assert!(!flags.document_quotas);
        assert!(!flags.document_object_deletion);
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| { Ok((key == KEY).then(|| "TRUE".to_owned())) })
            .is_err()
    );
}
