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
fn direct_oauth_resource_is_optional_and_requires_a_fixed_secure_audience() {
    let key = "CHRONO_NYXID_OAUTH_API_RESOURCE";
    assert!(configured(&[]).unwrap().nyxid_oauth_api_resource.is_none());
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
