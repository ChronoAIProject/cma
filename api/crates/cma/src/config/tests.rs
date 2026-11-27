use super::home;
include!("../config_tests.rs");
#[path = "erasure_tests.rs"]
mod erasure;
#[path = "machine_pool_tests.rs"]
mod machine_pools;
#[path = "../config_native_tests.rs"]
mod native;
#[path = "runtime_canary_tests.rs"]
mod runtime_canaries;
pub(crate) use runtime_canaries::{desktop_canary_config, retained_runtime_config};
#[path = "repository_runtime_images_tests.rs"]
mod repository_runtime_images;
mod home_tests {
    include!("home_tests.rs");
}

#[test]
fn startup_loads_bundled_switches_through_their_existing_readers() {
    let config = configured(&[(
        "CHRONO_FEATURE_SWITCHES",
        concat!(
            "CHRONO_GITHUB_CONNECTION_APPROVAL_ENABLED=true, ",
            "CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED=true, ",
            "CHRONO_V2_UI_TRANSPORT_ENABLED=false, ",
            "CHRONO_AGENT_MODEL_CREDIT_REQUIRED=true, ",
            "CHRONO_AGENT_SESSION_GUIDANCE_V8_ENABLED=true, ",
            "CHRONO_AM_BATCH_OWNERSHIP_RENEWAL=true",
        ),
    )])
    .unwrap();
    assert!(config.features.github_connection_approval);
    assert!(config.rate_limits.response_input_tier_enabled);
    assert!(!config.features.v2_ui_transport);
    assert!(config.features.model_credit_required);
    assert!(config.features.session_guidance_v8);
    assert!(config.features.batch_ownership_renewal);
}

#[test]
fn startup_gives_individual_switches_precedence() {
    let config = configured(&[
        (
            "CHRONO_FEATURE_SWITCHES",
            "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=true,CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED=false",
        ),
        ("CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED", "false"),
        ("CHRONO_SERVER_RATE_RESPONSE_INPUT_ENABLED", "true"),
    ])
    .unwrap();
    assert!(!config.features.bot_effect_reservations);
    assert!(config.rate_limits.response_input_tier_enabled);
    let error = configured(&[
        (
            "CHRONO_FEATURE_SWITCHES",
            "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=true",
        ),
        ("CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED", ""),
    ])
    .unwrap_err();
    assert_eq!(
        error,
        "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED must be true or false"
    );
}

#[test]
fn startup_preserves_defaults_with_an_empty_bundle() {
    let defaults = configured(&[]).unwrap();
    let empty = configured(&[("CHRONO_FEATURE_SWITCHES", "")]).unwrap();
    assert_eq!(empty.features, defaults.features);
    assert_eq!(
        empty.rate_limits.response_input_tier_enabled,
        defaults.rate_limits.response_input_tier_enabled
    );
}

#[test]
fn startup_refuses_malformed_or_duplicate_bundled_switches() {
    for invalid in [
        "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED",
        "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=invalid-marker",
        "CHRONO_MONGODB_DATABASE=true",
        "OTHER_SWITCH_ENABLED=true",
        "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=true,",
        "CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=true, CHRONO_BOT_EFFECT_RESERVATIONS_ENABLED=false",
    ] {
        let error = configured(&[("CHRONO_FEATURE_SWITCHES", invalid)]).unwrap_err();
        assert!(error.starts_with("CHRONO_FEATURE_SWITCHES entry "));
        assert!(!error.contains(invalid));
    }
}

#[test]
fn startup_rejects_bundled_switches_with_independent_environment_readers() {
    for name in [
        "CHRONO_AGENT_MANAGER_REPLICAS_ENABLED",
        "CHRONO_AGENT_MANAGER_OWNERSHIP_DIAGNOSTICS_ENABLED",
        "CHRONO_MONGO_DIAGNOSTICS_ENABLED",
    ] {
        let bundle = format!("{name}=true");
        let error = configured(&[("CHRONO_FEATURE_SWITCHES", &bundle)]).unwrap_err();
        assert_eq!(
            error,
            "CHRONO_FEATURE_SWITCHES entry 1 has an unsupported switch"
        );
    }
}

#[test]
fn startup_validates_cross_switch_prerequisites_for_bundled_values() {
    let error = configured(&[(
        "CHRONO_FEATURE_SWITCHES",
        "CHRONO_HOME_BACKUP_EXPIRY_ENABLED=true",
    )])
    .unwrap_err();
    assert!(error.contains("requires a qualified deletion provider"));
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

#[test]
fn proxy_replay_principal_writer_requires_explicit_activation() {
    for (value, expected) in [(None, false), (Some("false"), false), (Some("true"), true)] {
        let flags = super::FeatureFlags::from_lookup(&mut |key| {
            Ok((key == "CHRONO_NYXID_PROXY_REPLAY_PRINCIPAL_ENABLED")
                .then(|| value.map(str::to_owned))
                .flatten())
        })
        .unwrap();
        assert_eq!(flags.nyxid_proxy_replay_principal, expected);
    }
    for value in ["", "1", "TRUE", " true "] {
        assert!(super::FeatureFlags::from_lookup(&mut |key| Ok((key
            == "CHRONO_NYXID_PROXY_REPLAY_PRINCIPAL_ENABLED")
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

#[test]
fn privacy_export_requires_explicit_opt_in() {
    let key = "CHRONO_ACCOUNT_PRIVACY_EXPORT_ENABLED";
    assert!(!configured(&[]).unwrap().features.privacy_export_enabled);
    assert!(
        !configured(&[(key, "false")])
            .unwrap()
            .features
            .privacy_export_enabled
    );
    assert!(
        configured(&[(key, "true")])
            .unwrap()
            .features
            .privacy_export_enabled
    );
    assert!(configured(&[(key, "TRUE")]).is_err());
}

#[test]
fn direct_oauth_resource_is_optional_and_requires_a_fixed_secure_audience() {
    let key = "CHRONO_NYXID_OAUTH_API_RESOURCE";
    assert!(configured(&[]).unwrap().nyxid_oauth_api_resource.is_none());
    assert!(configured(&[(key, "")])
        .unwrap()
        .nyxid_oauth_api_resource
        .is_none());
    assert_eq!(
        configured(&[(key, "https://issuer.example/api/v1/proxy/s/cma")])
            .unwrap()
            .nyxid_oauth_api_resource
            .as_deref(),
        Some("https://issuer.example/api/v1/proxy/s/cma")
    );
    for value in [
        "not-url",
        "http://untrusted.example/cma",
        "https://user:secret@issuer.example/cma",
        "https://issuer.example/cma?target=other",
        "https://issuer.example/cma#fragment",
    ] {
        assert!(configured(&[(key, value)]).is_err());
    }
}

#[test]
fn control_rate_rejects_invalid_values_at_startup() {
    let key = "CHRONO_SERVER_RATE_CONTROL_PER_MINUTE";
    for invalid in ["", "0", "-1", "1.5", "nope", " 20", "4294967296"] {
        let error = configured(&[(key, invalid)]).unwrap_err();
        assert!(error.contains(key), "{error}");
    }
}

#[test]
fn control_rate_defaults_to_120_and_accepts_positive_u32_values() {
    let defaults = configured(&[]).unwrap().rate_limits;
    assert!(!defaults.control_tier_enabled);
    assert_eq!(defaults.control_per_minute.get(), 120);
    assert_eq!(defaults.create_per_minute.get(), 10);
    assert_eq!(defaults.input_per_minute.get(), 120);
    assert!(!defaults.response_input_tier_enabled);
    let key = "CHRONO_SERVER_RATE_CONTROL_PER_MINUTE";
    for value in [1, 20, 120, u32::MAX] {
        let raw = value.to_string();
        let limits = configured(&[(key, &raw)]).unwrap().rate_limits;
        assert_eq!(limits.control_per_minute.get(), value);
        assert_eq!(
            limits.create_per_minute.get(),
            defaults.create_per_minute.get()
        );
        assert_eq!(
            limits.input_per_minute.get(),
            defaults.input_per_minute.get()
        );
        assert_eq!(
            limits.response_input_tier_enabled,
            defaults.response_input_tier_enabled
        );
    }
}

#[test]
fn control_policy_requires_an_explicit_strict_boolean() {
    let key = "CHRONO_SERVER_RATE_CONTROL_ENABLED";
    for (value, expected) in [("false", false), ("true", true)] {
        assert_eq!(
            configured(&[(key, value)])
                .unwrap()
                .rate_limits
                .control_tier_enabled,
            expected
        );
    }
    for invalid in ["", "1", "TRUE", " true", "false "] {
        assert!(configured(&[(key, invalid)]).unwrap_err().contains(key));
    }
}

#[test]
fn activity_async_configuration_reaches_composition_and_rejects_invalid_values() {
    assert!(
        configured(&[("CHRONO_ACTIVITY_ASYNC_ENABLED", "true")])
            .unwrap()
            .activity_async_enabled
    );
    assert!(
        !configured(&[("CHRONO_ACTIVITY_ASYNC_ENABLED", "false")])
            .unwrap()
            .activity_async_enabled
    );
    assert!(configured(&[("CHRONO_ACTIVITY_ASYNC_ENABLED", "TRUE")]).is_err());
}

#[test]
fn shared_observation_requires_routed_replicas_and_bounded_replay() {
    let flags = [
        ("CHRONO_AGENT_SHARED_OBSERVATION_ENABLED", "true"),
        ("CHRONO_AGUI_BOUNDED_REPLAY_ENABLED", "true"),
    ];
    assert!(configured(&flags)
        .unwrap_err()
        .contains("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED"));
    let mut routed = flags.to_vec();
    routed.extend([
        ("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED", "true"),
        (
            "CHRONO_AGENT_MANAGER_RELAY_ADDRESS",
            "http://127.0.0.1:3000",
        ),
    ]);
    assert!(
        configured(&routed)
            .unwrap()
            .features
            .agent_shared_observation
    );
}

#[test]
fn home_backup_expiry_cannot_enable_without_a_qualified_production_provider() {
    let key = "CHRONO_HOME_BACKUP_EXPIRY_ENABLED";
    assert!(!configured(&[]).unwrap().features.home_backup_expiry_enabled);
    assert!(
        !configured(&[(key, "false")])
            .unwrap()
            .features
            .home_backup_expiry_enabled
    );
    let error = configured(&[(key, "true")]).unwrap_err();
    assert!(error.contains(key));
    assert!(error.contains("qualified deletion provider"));
}
