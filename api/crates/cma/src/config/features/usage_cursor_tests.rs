use super::FeatureFlags;

const ISSUE: &str = "CHRONO_USAGE_SIGNED_CURSORS_ENABLED";
const SNAPSHOT: &str = "CHRONO_USAGE_SNAPSHOT_CURSORS_ENABLED";
const STRICT: &str = "CHRONO_USAGE_STRICT_CURSORS_ENABLED";

#[test]
fn usage_cursor_rollout_is_default_off_and_strict_requires_signed_issuance() {
    let defaults = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
    assert!(!defaults.usage_signed_cursors_enabled && !defaults.usage_strict_cursors_enabled);
    for (issue, strict, valid) in [
        (false, false, true),
        (true, false, true),
        (true, true, true),
        (false, true, false),
    ] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok(match name {
                ISSUE => Some(issue.to_string()),
                STRICT => Some(strict.to_string()),
                _ => None,
            })
        });
        assert_eq!(flags.is_ok(), valid);
        if let Ok(flags) = flags {
            assert_eq!(flags.usage_signed_cursors_enabled, issue);
            assert_eq!(flags.usage_strict_cursors_enabled, strict);
        }
    }
}

#[test]
fn usage_cursor_flags_refuse_invalid_values_and_lookup_failures() {
    for name in [ISSUE, SNAPSHOT, STRICT] {
        for value in ["", "1", "TRUE", "yes", " true"] {
            let error =
                FeatureFlags::from_lookup(&mut |key| Ok((key == name).then(|| value.into())))
                    .unwrap_err();
            assert_eq!(error, format!("{name} must be true or false"));
        }
        assert!(FeatureFlags::from_lookup(&mut |key| {
            if key == name {
                Err("lookup unavailable".into())
            } else {
                Ok(None)
            }
        })
        .is_err());
    }
}

#[test]
fn usage_snapshot_issuance_is_default_off_and_requires_signed_issuance() {
    let defaults = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
    assert!(!defaults.usage_snapshot_cursors_enabled);
    let error = FeatureFlags::from_lookup(&mut |key| Ok((key == SNAPSHOT).then(|| "true".into())))
        .unwrap_err();
    assert_eq!(error, format!("{SNAPSHOT} requires {ISSUE}"));
    let flags = FeatureFlags::from_lookup(&mut |key| {
        Ok([ISSUE, SNAPSHOT].contains(&key).then(|| "true".into()))
    })
    .unwrap();
    assert!(flags.usage_snapshot_cursors_enabled);
}
