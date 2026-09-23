use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use uuid::Uuid;

use super::*;

const TEST_API_KEY: &str = "sandbox-secret-never-print";
const TEST_IMAGE: &str = concat!(
    "registry.example/cma-runtime:v1@sha256:",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaa"
);
const REQUIRED_RUNTIME_KEYS: [&str; 8] = [
    "CHRONO_SANDBOX_BASE_URL",
    "CHRONO_SANDBOX_API_KEY_FILE",
    "CHRONO_SANDBOX_IMAGE",
    "CHRONO_SANDBOX_NAMESPACE",
    "CHRONO_UPSTREAM_BASE_URL",
    "CHRONO_UPSTREAM_WIRE_API",
    "CHRONO_PLATFORM_BASE_URL",
    "CHRONO_DEPLOYMENT_ID",
];

struct TemporaryEntry {
    path: PathBuf,
}

impl TemporaryEntry {
    fn file(contents: &[u8]) -> Self {
        let entry = Self::missing();
        fs::write(&entry.path, contents).expect("write temporary configuration secret");
        entry
    }

    fn directory() -> Self {
        let entry = Self::missing();
        fs::create_dir(&entry.path).expect("create temporary configuration directory");
        entry
    }

    fn missing() -> Self {
        Self {
            path: std::env::temp_dir().join(format!("cma-config-{}", Uuid::new_v4())),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn path_str(&self) -> &str {
        self.path.to_str().expect("temporary path is Unicode")
    }
}

impl Drop for TemporaryEntry {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.path);
    }
}

fn configured(overrides: &[(&str, &str)]) -> Result<Config, String> {
    configured_with_removed_key(overrides, None)
}

fn configured_without(key: &str) -> Result<Config, String> {
    configured_with_removed_key(&[], Some(key))
}

fn configured_with_removed_key(
    overrides: &[(&str, &str)],
    removed_key: Option<&str>,
) -> Result<Config, String> {
    let secret = TemporaryEntry::file(format!("\n  {TEST_API_KEY}\t\n").as_bytes());
    let mut values = HashMap::from([
        (
            "CHRONO_CHECKPOINT_BUCKET".to_owned(),
            "cma-checkpoint-test".to_owned(),
        ),
        (
            "CHRONO_STORAGE_CLIENT_ID".to_owned(),
            "storage-test-client".to_owned(),
        ),
        (
            "CHRONO_STORAGE_CLIENT_SECRET".to_owned(),
            "storage-identity-material".to_owned(),
        ),
        ("CHRONO_AUTH_MODE".to_owned(), "trusted".to_owned()),
        (
            "CHRONO_MONGODB_URI".to_owned(),
            "mongodb://localhost/cma".to_owned(),
        ),
        (
            "CHRONO_AUTH_ENCRYPTION_KEY".to_owned(),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        ),
        (
            "CHRONO_SANDBOX_BASE_URL".to_owned(),
            "http://sandbox.example".to_owned(),
        ),
        (
            "CHRONO_SANDBOX_API_KEY_FILE".to_owned(),
            secret.path().to_string_lossy().into_owned(),
        ),
        ("CHRONO_SANDBOX_IMAGE".to_owned(), TEST_IMAGE.to_owned()),
        (
            "CHRONO_SANDBOX_NAMESPACE".to_owned(),
            "chrono-bot".to_owned(),
        ),
        (
            "CHRONO_UPSTREAM_BASE_URL".to_owned(),
            "https://models.example/api/v1/proxy/s/model".to_owned(),
        ),
        (
            "CHRONO_UPSTREAM_WIRE_API".to_owned(),
            "responses".to_owned(),
        ),
        (
            "CHRONO_PLATFORM_BASE_URL".to_owned(),
            "https://platform.example".to_owned(),
        ),
        ("CHRONO_DEPLOYMENT_ID".to_owned(), "cma-test".to_owned()),
    ]);
    values.extend(
        overrides
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
    );
    if let Some(key) = removed_key {
        values.remove(key);
    }
    Config::from_lookup(|key| Ok(values.get(key).cloned()))
}

#[test]
fn retained_defaults_and_runtime_values_form_a_complete_configuration() {
    let config = configured(&[]).unwrap();
    assert_eq!(config.bind, "0.0.0.0:8080".parse().unwrap());
    assert_eq!(config.mongodb_database, DEFAULT_DATABASE);
    assert_eq!(config.deployment_id, "cma-test");
    assert_eq!(config.activity_retention, Duration::from_secs(90 * 86_400));
    assert_eq!(config.default_profiles.cpu_millis, 1_000);
    assert_eq!(config.default_profiles.memory_mib, 1_024);
    assert_eq!(
        config
            .baseline_services
            .iter()
            .map(ServiceId::as_str)
            .collect::<Vec<_>>(),
        ["chrono-llm-public", "ornn-api"]
    );
    assert_eq!(config.auth_mode_name(), "trusted");
    assert_eq!(config.runtime.sandbox_base_url, "http://sandbox.example");
    assert_eq!(config.runtime.sandbox_api_key, TEST_API_KEY);
    assert_eq!(config.runtime.sandbox_image, TEST_IMAGE);
    assert_eq!(config.runtime.sandbox_namespace, "chrono-bot");
    assert_eq!(
        config.runtime.upstream_base_url,
        "https://models.example/api/v1/proxy/s/model"
    );
    assert_eq!(config.runtime.upstream_wire_api, "responses");
    assert_eq!(config.runtime.platform_base_url, "https://platform.example");

    let debug = format!("{config:?}");
    assert!(!debug.contains("mongodb://localhost"));
    assert!(!debug.contains(TEST_API_KEY));
    assert!(!format!("{:?}", config.runtime).contains(TEST_API_KEY));
}

#[test]
fn runtime_values_are_required_and_nonblank() {
    for key in REQUIRED_RUNTIME_KEYS {
        let missing = configured_without(key).unwrap_err();
        assert!(
            missing.contains(key),
            "unexpected error for {key}: {missing}"
        );

        let blank = configured(&[(key, " \t")]).unwrap_err();
        assert!(blank.contains(key), "unexpected error for {key}: {blank}");
    }
}

#[test]
fn checkpoint_storage_identity_and_bucket_are_required_and_redacted() {
    for key in [
        "CHRONO_CHECKPOINT_BUCKET",
        "CHRONO_STORAGE_CLIENT_ID",
        "CHRONO_STORAGE_CLIENT_SECRET",
    ] {
        assert!(configured_without(key).unwrap_err().contains(key));
        assert!(configured(&[(key, " \t")]).unwrap_err().contains(key));
    }
    for bucket in [
        "ab",
        "Uppercase",
        "-starts",
        "ends-",
        "with/slash",
        "with.dot",
        " padded",
    ] {
        assert!(configured(&[("CHRONO_CHECKPOINT_BUCKET", bucket)]).is_err());
    }
    for key in ["CHRONO_STORAGE_CLIENT_ID", "CHRONO_STORAGE_CLIENT_SECRET"] {
        for value in [" padded", "padded ", "with\ncontrol"] {
            let error = configured(&[(key, value)]).unwrap_err();
            assert!(error.contains(key));
            assert!(!error.contains(value));
        }
    }
    assert!(configured(&[("CHRONO_CHECKPOINT_BUCKET", &"a".repeat(64))]).is_err());
    assert!(configured(&[("CHRONO_STORAGE_CLIENT_ID", &"a".repeat(257))]).is_err());
    assert!(configured(&[("CHRONO_STORAGE_CLIENT_SECRET", &"a".repeat(4097))]).is_err());
    let config = configured(&[]).unwrap();
    assert_eq!(config.checkpoint_storage.bucket, "cma-checkpoint-test");
    assert!(!format!("{config:?}").contains("storage-identity-material"));
    assert!(!format!("{:?}", config.checkpoint_storage).contains("storage-identity-material"));
}

#[test]
fn runtime_urls_are_canonical_and_restrict_transport() {
    for value in [
        "sandbox.example",
        "ftp://sandbox.example",
        "http://Sandbox.example",
        "http://user@sandbox.example",
        "http://sandbox.example/path",
        "http://sandbox.example?query",
        "http://sandbox.example/",
    ] {
        assert!(configured(&[("CHRONO_SANDBOX_BASE_URL", value)]).is_err());
    }
    for value in [
        "http://models.example/v1",
        "https://Models.example/v1",
        "https://user@models.example/v1",
        "https://models.example/v1?query",
        "https://models.example/v1/",
    ] {
        assert!(configured(&[("CHRONO_UPSTREAM_BASE_URL", value)]).is_err());
    }
    for value in [
        "http://platform.example",
        "https://Platform.example",
        "https://platform.example/",
        "https://platform.example/path",
        "https://platform.example#fragment",
    ] {
        assert!(configured(&[("CHRONO_PLATFORM_BASE_URL", value)]).is_err());
    }
}

#[test]
fn runtime_wire_api_is_the_demonstrated_contract() {
    let error = configured(&[("CHRONO_UPSTREAM_WIRE_API", "chat_completions")]).unwrap_err();
    assert!(error.contains("CHRONO_UPSTREAM_WIRE_API"));
    assert!(error.contains("responses"));
}

#[test]
fn runtime_image_is_an_exact_sha256_pinned_reference() {
    for value in [
        "registry.example/cma-runtime:latest",
        concat!(
            "@sha256:",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
        concat!(
            "https://registry.example/runtime@sha256:",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
        concat!(
            "registry.example/runtime@sha256:",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        ),
        concat!(
            "registry.example/runtime@sha256:",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
    ] {
        let error = configured(&[("CHRONO_SANDBOX_IMAGE", value)]).unwrap_err();
        assert!(error.contains("CHRONO_SANDBOX_IMAGE"));
    }
}

#[test]
fn sandbox_namespace_is_one_dns_1123_label() {
    for value in [
        "Chrono-bot",
        "chrono.bot",
        "-chrono-bot",
        "chrono-bot-",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let error = configured(&[("CHRONO_SANDBOX_NAMESPACE", value)]).unwrap_err();
        assert!(error.contains("CHRONO_SANDBOX_NAMESPACE"));
    }
}

#[test]
fn deployment_id_is_a_nonempty_kubernetes_label_value() {
    assert!(configured(&[("CHRONO_DEPLOYMENT_ID", "CMA.prod_1")]).is_ok());
    for value in [
        "-cma",
        "cma_",
        "cma/control-plane",
        "cma control plane",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        let error = configured(&[("CHRONO_DEPLOYMENT_ID", value)]).unwrap_err();
        assert!(error.contains("CHRONO_DEPLOYMENT_ID"));
    }
}

#[test]
fn sandbox_api_key_file_fails_closed_without_disclosing_contents() {
    let missing = TemporaryEntry::missing();
    let error = configured(&[("CHRONO_SANDBOX_API_KEY_FILE", missing.path_str())]).unwrap_err();
    assert!(error.contains("CHRONO_SANDBOX_API_KEY_FILE"));

    let directory = TemporaryEntry::directory();
    let error = configured(&[("CHRONO_SANDBOX_API_KEY_FILE", directory.path_str())]).unwrap_err();
    assert!(error.contains("CHRONO_SANDBOX_API_KEY_FILE"));

    let invalid_utf8 = TemporaryEntry::file(b"do-not-print-this\xff");
    let error =
        configured(&[("CHRONO_SANDBOX_API_KEY_FILE", invalid_utf8.path_str())]).unwrap_err();
    assert!(error.contains("valid UTF-8"));
    assert!(!error.contains("do-not-print-this"));

    let empty = TemporaryEntry::file(b" \n\t ");
    let error = configured(&[("CHRONO_SANDBOX_API_KEY_FILE", empty.path_str())]).unwrap_err();
    assert!(error.contains("nonempty API key"));

    let invalid_header = TemporaryEntry::file(b"secret-do-not-print\ninjected-header");
    let error =
        configured(&[("CHRONO_SANDBOX_API_KEY_FILE", invalid_header.path_str())]).unwrap_err();
    assert!(error.contains("valid HTTP header value"));
    assert!(!error.contains("secret-do-not-print"));
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

#[test]
fn interface_feature_flags_are_strict_independent_booleans() {
    let config = configured(&[]).expect("existing configuration starts");
    assert!(!config.features.v2_ui_transport);
    assert!(!config.features.trigger_command_admission);
    for key in [
        "CHRONO_V2_UI_TRANSPORT_ENABLED",
        "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED",
    ] {
        let config = configured(&[(key, "true")]).unwrap();
        assert_eq!(
            config.features.v2_ui_transport,
            key == "CHRONO_V2_UI_TRANSPORT_ENABLED"
        );
        assert_eq!(
            config.features.trigger_command_admission,
            key == "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED"
        );
        assert_eq!(
            configured(&[(key, "enabled")]).unwrap_err(),
            format!("{key} must be true or false")
        );
    }
}
