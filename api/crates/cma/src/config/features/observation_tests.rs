use super::*;
const FLAG: &str = "CHRONO_AGENT_SHARED_OBSERVATION_ENABLED";
#[test]
fn shared_observation_is_closed_strict_and_requires_bounded_replay() {
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .agent_shared_observation
    );
    for value in ["", "1", "TRUE", "true "] {
        assert!(
            FeatureFlags::from_lookup(&mut |key| Ok((key == FLAG).then(|| value.into()))).is_err()
        );
    }
    assert!(
        FeatureFlags::from_lookup(&mut |key| Ok((key == FLAG).then(|| "true".into()))).is_err()
    );
    let flags = FeatureFlags::from_lookup(&mut |key| {
        Ok(matches!(key, FLAG | "CHRONO_AGUI_BOUNDED_REPLAY_ENABLED").then(|| "true".into()))
    })
    .unwrap();
    assert!(flags.agent_shared_observation);
    assert!(
        FeatureFlags::from_lookup(&mut |key| Ok((key == FLAG).then(|| "false".into()))).is_ok()
    );
}
