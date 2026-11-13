use super::FeatureFlags;
const KEY: &str = "CHRONO_SANDBOX_AGENT_PROJECTION_ENABLED";

#[test]
fn agent_projection_is_independent_strict_and_default_off() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then_some(value).flatten().map(str::to_owned))
        })
        .unwrap();
        assert_eq!(flags.sandbox_agent_projection, expected);
        assert!(!flags.access_snapshot_session);
        assert!(!flags.concurrent_sandbox_detail_enrichment);
        assert!(!flags.sandbox_snapshot_reads);
        assert!(!flags.sandbox_batched_reads);
    }
    for value in ["", "1", "TRUE", "False", " true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == KEY).then(|| value.into()))).is_err()
        );
    }
    assert!(FeatureFlags::from_lookup(&mut |key| if key == KEY {
        Err("lookup failed".into())
    } else {
        Ok(None)
    })
    .is_err());
}
