use super::{FeatureFlags, GRADUATED};

const KEY: &str = "CHRONO_AGENT_MANAGER_FAILURE_CONTAINMENT_ENABLED";

#[test]
fn failure_containment_requires_its_own_strict_default_off_policy() {
    assert!(!GRADUATED.contains(&KEY));
    assert!(!FeatureFlags::default().am_failure_containment_enabled);
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.am_failure_containment_enabled, enabled);
        let legacy = FeatureFlags::from_lookup(&mut |_| Ok(None)).unwrap();
        assert_eq!(
            flags,
            FeatureFlags {
                am_failure_containment_enabled: enabled,
                ..legacy
            }
        );
    }
    for value in ["", "1", "TRUE", " true "] {
        let result =
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.to_owned())));
        assert_eq!(result.unwrap_err(), format!("{KEY} must be true or false"));
    }
}

#[test]
fn failure_containment_lookup_failure_is_not_silently_disabled() {
    let result = FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("synthetic configuration unavailable".into())
        } else {
            Ok(None)
        }
    });
    assert_eq!(result.unwrap_err(), "synthetic configuration unavailable");
}
