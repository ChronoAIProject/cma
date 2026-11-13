use super::*;
const KEY: &str = "CHRONO_AGENT_LOG_SCAN_COOPERATIVE_ENABLED";

#[test]
fn cooperation_is_default_off_and_independent_of_other_feature_flags() {
    let baseline = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
    assert!(!baseline.agent_log_scan_cooperative_enabled);
    for enabled in [false, true] {
        let flags =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| enabled.to_string())))
                .unwrap();
        let mut expected = baseline;
        expected.agent_log_scan_cooperative_enabled = enabled;
        assert_eq!(flags, expected);
    }
}

#[test]
fn cooperation_rejects_non_boolean_values_and_preserves_lookup_failures() {
    for invalid in ["", "1", "TRUE", "false ", "yes"] {
        let error =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| invalid.to_owned())))
                .unwrap_err();
        assert_eq!(error, format!("{KEY} must be true or false"));
    }
    let error = FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("lookup refused".into())
        } else {
            Ok(None)
        }
    })
    .unwrap_err();
    assert_eq!(error, "lookup refused");
}
