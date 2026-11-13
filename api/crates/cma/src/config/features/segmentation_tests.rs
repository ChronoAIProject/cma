use super::FeatureFlags;

const USAGE: &str = "CHRONO_AGENT_USAGE_BOUNDED_READS_ENABLED";

#[test]
fn usage_opt_in_is_strict_and_does_not_enable_other_consumers() {
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == USAGE).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.bounded_usage_reads, enabled);
        assert!(!flags.bounded_transcript_reads);
        assert!(!flags.bounded_response_reads);
        assert!(!flags.paged_response_events);
        assert!(!flags.capacity_recovery_enabled);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == USAGE).then(|| value.to_owned()))
        })
        .is_err());
    }
}

#[test]
fn recovery_admission_requires_its_own_strict_default_off_switch() {
    const RECOVERY: &str = "CHRONO_AGENT_CAPACITY_RECOVERY_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == RECOVERY)
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.capacity_recovery_enabled, enabled);
        assert!(!flags.bounded_usage_reads);
        assert!(!flags.bounded_transcript_reads);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == RECOVERY).then(|| value.to_owned()))
        })
        .is_err());
    }
}

#[test]
fn missing_usage_configuration_access_is_a_startup_error() {
    let error = FeatureFlags::from_lookup(&mut |key| {
        if key == USAGE {
            Err("configuration lookup unavailable".into())
        } else {
            Ok(None)
        }
    })
    .unwrap_err();
    assert_eq!(error, "configuration lookup unavailable");
}
#[test]
fn segment_adoption_requires_an_independent_strict_process_opt_in() {
    const KEY: &str = "CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED";
    for (value, enabled) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.map(str::to_owned)).flatten())
        })
        .unwrap();
        assert_eq!(flags.segment_adoption_enabled, enabled);
        assert!(!flags.capacity_recovery_enabled);
        assert!(!flags.bounded_usage_reads);
        assert!(!flags.bounded_transcript_reads);
        assert!(!flags.bounded_response_reads);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(FeatureFlags::from_lookup(&mut |key| {
            Ok((key == KEY).then(|| value.to_owned()))
        })
        .is_err());
    }
    assert!(FeatureFlags::from_lookup(&mut |key| {
        if key == KEY {
            Err("unavailable".into())
        } else {
            Ok(None)
        }
    })
    .is_err());
}
