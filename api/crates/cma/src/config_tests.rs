use std::collections::HashMap;

use super::*;

fn configured(overrides: &[(&str, &str)]) -> Result<Config, String> {
    let mut values = HashMap::from([
        ("CHRONO_AUTH_MODE", "trusted"),
        ("CHRONO_MONGODB_URI", "mongodb://localhost/cma"),
        (
            "CHRONO_AUTH_ENCRYPTION_KEY",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        ),
    ]);
    values.extend(overrides.iter().copied());
    Config::from_lookup(|key| Ok(values.get(key).map(ToString::to_string)))
}

#[test]
fn retained_defaults_form_a_complete_configuration() {
    let config = configured(&[]).unwrap();
    assert_eq!(config.bind, "0.0.0.0:8080".parse().unwrap());
    assert_eq!(config.mongodb_database, DEFAULT_DATABASE);
    assert_eq!(config.activity_retention, Duration::from_secs(90 * 86_400));
    assert_eq!(config.default_profiles.cpu_millis, 1_000);
    assert_eq!(config.default_profiles.memory_mib, 1_024);
    assert_eq!(
        config
            .baseline_services
            .iter()
            .map(ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["chrono-llm", "ornn", "ornn-api"]
    );
    assert_eq!(config.auth_mode_name(), "trusted");
    assert!(!format!("{config:?}").contains("mongodb://localhost"));
}

#[test]
fn baseline_services_are_nonempty_valid_and_deduplicated() {
    let config = configured(&[("CHRONO_BASELINE_SERVICES", " model,api-github,model ")])
        .expect("valid services");
    assert_eq!(
        config
            .baseline_services
            .iter()
            .map(ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["api-github", "model"]
    );
    for value in ["", ",,", "service with spaces", "bad/service"] {
        assert!(configured(&[("CHRONO_BASELINE_SERVICES", value)]).is_err());
    }
}

#[test]
fn secrets_and_unsafe_network_configuration_fail_closed() {
    assert!(configured(&[("CHRONO_AUTH_ENCRYPTION_KEY", "short")]).is_err());
    assert!(configured(&[("CHRONO_SERVER_RATE_HEALTH_PER_MINUTE", "29")]).is_err());
    assert!(configured(&[("CHRONO_SERVER_PUBLIC_URL", "https://user@example.com/path",)]).is_err());
    assert!(configured(&[("CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS", "192.0.2.0/24")]).is_err());
}

#[test]
fn provider_mode_requires_all_identity_configuration() {
    assert!(configured(&[("CHRONO_AUTH_MODE", "provider")]).is_err());
    let config = configured(&[
        ("CHRONO_AUTH_MODE", "provider"),
        ("CHRONO_NYXID_BASE_URL", "https://identity.example"),
        ("CHRONO_NYXID_PUBLIC_URL", "https://nyx.example"),
        ("CHRONO_NYXID_CLIENT_ID", "client"),
        ("CHRONO_NYXID_CLIENT_SECRET", "never-print-me"),
    ])
    .unwrap();
    assert_eq!(config.auth_mode_name(), "provider");
    assert_eq!(
        config.nyxid_public_url.as_deref(),
        Some("https://nyx.example")
    );
    assert!(!format!("{config:?}").contains("never-print-me"));
}

#[test]
fn nyxid_browser_origin_is_canonical_https() {
    for value in [
        "http://nyx.example",
        "https://Nyx.example",
        "https://nyx.example/",
        "https://user@nyx.example",
        "https://nyx.example/register",
    ] {
        assert!(parse_canonical_https_origin("CHRONO_NYXID_PUBLIC_URL", value).is_err());
    }
}
