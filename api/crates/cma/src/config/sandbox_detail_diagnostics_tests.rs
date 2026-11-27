use super::*;

#[test]
fn sandbox_snapshot_is_opt_in_and_detail_timings_are_default_on() {
    for key in [
        "CHRONO_SANDBOX_SNAPSHOT_READS_ENABLED",
        "CHRONO_SANDBOX_DETAIL_TIMINGS_ENABLED",
    ] {
        for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then(|| value.map(str::to_owned)).flatten())
            })
            .unwrap();
            assert_eq!(
                flags.sandbox_snapshot_reads,
                enabled && key.contains("SNAPSHOT")
            );
            assert_eq!(
                flags.sandbox_detail_timings,
                if key.contains("TIMINGS") {
                    value != Some("false")
                } else {
                    true
                }
            );
        }
        for invalid in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |name| {
                Ok((name == key).then(|| invalid.to_owned()))
            })
            .is_err());
        }
    }
}

#[test]
fn agent_file_read_timings_are_default_off_and_strict() {
    let key = "CHRONO_AGENT_FILE_READ_TIMINGS_ENABLED";
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |name| {
            Ok(if name == key {
                value.map(str::to_owned)
            } else {
                None
            })
        })
        .unwrap();
        assert_eq!(flags.agent_file_read_timings, expected);
    }
    for invalid in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |name| Ok(
            (name == key).then(|| invalid.to_owned())
        ))
        .is_err());
    }
}

#[test]
fn single_exchange_file_reads_are_default_off_and_strict() {
    const KEY: &str = "CHRONO_AGENT_FILE_SINGLE_EXCHANGE_ENABLED";
    assert!(
        !FeatureFlags::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .agent_file_single_exchange
    );
    assert!(
        FeatureFlags::from_lookup(&mut |name| Ok((name == KEY).then(|| "true".into())))
            .unwrap()
            .agent_file_single_exchange
    );
    assert!(
        FeatureFlags::from_lookup(&mut |name| Ok((name == KEY).then(|| "TRUE".into()))).is_err()
    );
}
