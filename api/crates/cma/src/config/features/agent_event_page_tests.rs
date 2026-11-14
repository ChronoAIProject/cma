use super::FeatureFlags;

const KEY: &str = "CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED";

#[test]
fn buffered_event_page_switch_is_independent_default_off_and_strict() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.single_fence_event_pages, expected);
        assert!(!flags.access_snapshot_session);
    }
    for invalid in ["", "TRUE", "1", " true", "false "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| invalid.into()))).is_err()
        );
    }
    assert_eq!(
        FeatureFlags::from_lookup(&mut |key| {
            if key == KEY {
                Err("lookup failed".into())
            } else {
                Ok(None)
            }
        })
        .unwrap_err(),
        "lookup failed"
    );
    assert!(include_str!("../../../../../../infra/api/configmap.yaml")
        .contains("CHRONO_AGENT_EVENT_PAGE_SINGLE_FENCE_ENABLED: \"false\""));
}
