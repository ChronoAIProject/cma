use super::*;
#[test]
fn agent_discovery_switches_are_independent_strict_and_default_off() {
    let keys = [
        "CHRONO_AGENT_MODEL_DISCOVERY_CACHE_ENABLED",
        "CHRONO_AGENT_ENVIRONMENT_ASYNC_ENABLED",
    ];
    for key in keys {
        for (raw, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then_some(raw).flatten().map(str::to_owned))
            })
            .unwrap();
            assert_eq!(flags.agent_model_discovery_cache, enabled && key == keys[0]);
            assert_eq!(flags.agent_environment_async, enabled && key == keys[1]);
        }
        for raw in ["", "TRUE", "1", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |name| Ok(
                (name == key).then(|| raw.to_owned())
            ))
            .is_err());
        }
    }
}
