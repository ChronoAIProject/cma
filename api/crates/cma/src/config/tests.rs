use super::home;
include!("../config_tests.rs");
#[path = "machine_pool_tests.rs"]
mod machine_pools;
#[path = "../config_native_tests.rs"]
mod native;
mod home_tests {
    include!("home_tests.rs");
}

#[test]
fn response_input_tier_requires_explicit_boolean_and_preserves_existing_rates() {
    let key = "CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED";
    for (values, enabled) in [
        (vec![], false),
        (vec![(key, "false")], false),
        (vec![(key, "true")], true),
    ] {
        let limits = configured(&values).unwrap().rate_limits;
        assert_eq!(limits.response_input_tier_enabled, enabled);
        assert_eq!(limits.create_per_minute.get(), 10);
        assert_eq!(limits.input_per_minute.get(), 120);
    }
    for value in ["", "TRUE", "1", " true ", "yes"] {
        assert!(configured(&[(key, value)]).unwrap_err().contains(key));
    }
    assert_eq!(
        configured(&[(key, "true"), ("CHRONO_SERVER_RATE_INPUT_PER_MINUTE", "17")])
            .unwrap()
            .rate_limits
            .input_per_minute
            .get(),
        17
    );
}

#[test]
fn reporting_pool_setting_is_validated_by_startup_configuration() {
    assert_eq!(
        configured(&[]).unwrap().usage_metrics.reporting_pool_size,
        4
    );
    let key = "CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE";
    assert_eq!(
        configured(&[(key, "8")])
            .unwrap()
            .usage_metrics
            .reporting_pool_size,
        8
    );
    for value in ["", "0", "17", "invalid"] {
        assert!(configured(&[(key, value)]).unwrap_err().contains(key));
    }
}

#[test]
fn skill_upgrades_require_an_explicit_strict_boolean() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = super::FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_AGENT_SKILL_UPGRADE_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.agent_skill_upgrade_enabled, expected);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(super::FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_AGENT_SKILL_UPGRADE_ENABLED")
            .then(|| value.to_owned())))
        .is_err());
    }
}

#[cfg(test)]
mod silent_sign_in_tests {
    use super::*;
    #[test]
    fn silent_sign_in_requires_explicit_activation() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_SILENT_SIGN_IN_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.silent_sign_in, expected);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| Ok((key
                == "CHRONO_SILENT_SIGN_IN_ENABLED")
                .then(|| value.to_owned())))
            .is_err());
        }
    }
}

#[cfg(test)]
mod shared_output_tests {
    use super::*;

    #[test]
    fn shared_runtime_output_requires_its_own_strict_opt_in() {
        for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
            let flags = FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED")
                    .then(|| value.map(str::to_owned))
                    .flatten())
            })
            .unwrap();
            assert_eq!(flags.shared_runtime_output, expected);
            assert!(!flags.bounded_response_reads);
        }
        for value in ["", "1", "TRUE", " true "] {
            assert!(FeatureFlags::from_lookup(&mut |key| {
                Ok((key == "CHRONO_RUNTIME_SHARED_OUTPUT_ENABLED").then(|| value.into()))
            })
            .is_err());
        }
    }
}
