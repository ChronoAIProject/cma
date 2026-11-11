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
