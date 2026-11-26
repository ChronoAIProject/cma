//! Graduated rollout gates against staging's deployed Agent Manager
//! environment.
use super::{
    features::{self, GRADUATED},
    Config,
};
use std::{collections::HashMap, fs, path::PathBuf};

/// Staging's live literal environment, key for key. The infra-internal
/// cluster address is a documentation IP, the Pod IP that Kubernetes
/// substitutes into the relay address is fixed, and the deploy controller's
/// bootstrap receipts, which no reader consumes, hold placeholders.
const STAGING: &[(&str, &str)] = &[
    ("CHRONO_AGENT_MANAGER_RELAY_ADDRESS", "http://10.0.0.10:8080"),
    ("CHRONO_AGENT_MANAGER_REPLICAS_ENABLED", "true"),
    ("CHRONO_AGENT_SANDBOX_API_SERVER", "https://192.0.2.10"),
    ("CHRONO_AGENT_SANDBOX_AUTH_MODE", "google-metadata"),
    (
        "CHRONO_AGENT_SANDBOX_CA_FILE",
        "/etc/cma-staging/sandbox-kubernetes-ca.crt",
    ),
    (
        "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE",
        "/etc/cma-staging/agent-sandbox-control-key",
    ),
    ("CHRONO_AGENT_SANDBOX_ENABLED", "true"),
    ("CHRONO_AGENT_SANDBOX_HOME_CAPACITY_GIB", "64"),
    ("CHRONO_AGENT_SANDBOX_HOME_MODE", "native_vct_full_home_v2"),
    ("CHRONO_AGENT_SANDBOX_HOME_STORAGE_CLASS", "home-a"),
    ("CHRONO_AGENT_SANDBOX_NAMESPACE", "chrono-pod-staging"),
    ("CHRONO_AGENT_SANDBOX_RUNTIME_CLASS", "microvm"),
    ("CHRONO_AGENT_SANDBOX_RUNTIME_TRANSPORT", "kubernetes-exec"),
    ("CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT", "default"),
    ("CHRONO_AUTH_MODE", "provider"),
    (
        "CHRONO_CMAEG_BASE_URL",
        "http://cma-event-gateway.chrono-ai-staging.svc:8090",
    ),
    (
        "CHRONO_CMAEG_PUBLIC_API_URL",
        "https://cma-staging.nyxid.ai/event-gateway",
    ),
    ("CHRONO_CMAEG_PUBLIC_URL", "https://cmaeg-staging.nyxid.ai"),
    ("CHRONO_DEPLOYMENT_ID", "chrono-ai-staging"),
    (
        "CHRONO_MACHINE_SANDBOX_IMAGE",
        "ghcr.io/chronoaiproject/cma-runtime-omarchy@sha256:86c0193dbb96cba20b1194ec31677e0e7c9e840a62722a6978cbd8370cdbc53c",
    ),
    ("CHRONO_METRICS_BIND", "0.0.0.0:9091"),
    ("CHRONO_MONGODB_AUTHORITY_MODE", "shared-schema"),
    ("CHRONO_MONGODB_DATABASE", "cma"),
    ("CHRONO_NATIVE_MACHINE_PREPARED_TARGET", "0"),
    ("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET", "0"),
    ("CHRONO_NATIVE_PREPARED_TARGET", "2"),
    ("CHRONO_NATIVE_WARM_POOL_TARGET", "2"),
    ("CHRONO_NYXID_BASE_URL", "https://staging-api.nyxid.ai"),
    (
        "CHRONO_NYXID_OAUTH_API_RESOURCE",
        "https://staging-api.nyxid.ai/api/v1/proxy/s/cma",
    ),
    ("CHRONO_NYXID_PROXY_AUDIENCE", "https://cma-staging.nyxid.ai"),
    ("CHRONO_NYXID_PROXY_ENABLED", "false"),
    ("CHRONO_NYXID_PROXY_ISSUER", "https://staging-api.nyxid.ai"),
    (
        "CHRONO_NYXID_PROXY_JWKS_URL",
        "https://staging-api.nyxid.ai/.well-known/jwks.json",
    ),
    ("CHRONO_NYXID_PUBLIC_URL", "https://staging.nyxid.ai"),
    ("CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED", "true"),
    ("CHRONO_PLATFORM_BASE_URL", "https://staging-api.nyxid.ai"),
    ("CHRONO_RETAIN_OPENSANDBOX", "false"),
    (
        "CHRONO_SANDBOX_IMAGE",
        "ghcr.io/chronoaiproject/cma-runtime@sha256:fbd25499468d00bfc2e78a7b1c825316c0adb6932674cde73fc9556309361d3e",
    ),
    ("CHRONO_SANDBOX_NAMESPACE", "chrono-pod-staging"),
    ("CHRONO_SANDBOX_PROVIDER", "agent-sandbox"),
    ("CHRONO_SERVER_BIND", "0.0.0.0:8080"),
    ("CHRONO_SERVER_CORS_ORIGIN", "https://cma-staging.nyxid.ai"),
    ("CHRONO_SERVER_PUBLIC_URL", "https://cma-staging.nyxid.ai"),
    (
        "CHRONO_TASK_EXPERIENCE_POLICY",
        r#"{"mode":"all","accounts":[],"capabilities":["task_home_admission","suggested_followup_admission","replay_presentation","desktop_enable"]}"#,
    ),
    (
        "CHRONO_UPSTREAM_BASE_URL",
        "https://staging-api.nyxid.ai/api/v1/proxy/s/chrono-llm-public",
    ),
    ("CHRONO_UPSTREAM_WIRE_API", "responses"),
    ("CHRONO_USAGE_METRICS_ENABLED", "false"),
    ("CMA_CD_OMARCHY_BOOTSTRAP_RECEIPT", "placeholder"),
    ("CMA_CD_RUNTIME_BOOTSTRAP_RECEIPT", "placeholder"),
    ("RUST_MIN_STACK", "16777216"),
];

const CA_FILE: &str = "CHRONO_AGENT_SANDBOX_CA_FILE";
const CONTROL_KEY_FILE: &str = "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE";

/// Staging's live secret names, exactly.
const STAGING_SECRETS: &[&str] = &[
    "CHRONO_AUTH_ENCRYPTION_KEY",
    "CHRONO_MONGODB_URI",
    "CHRONO_NYXID_CLIENT_ID",
    "CHRONO_NYXID_CLIENT_SECRET",
    "CHRONO_NYXID_SERVICE_ID",
];

/// A temporary file standing in for a mounted one.
struct Mounted(PathBuf);

impl Mounted {
    fn new(contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cma-graduation-{}", uuid::Uuid::new_v4()));
        fs::write(&path, contents).expect("write mounted placeholder");
        Self(path)
    }

    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn placeholder(name: &str) -> String {
    match name {
        "CHRONO_AUTH_ENCRYPTION_KEY" => "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
        "CHRONO_MONGODB_URI" => "mongodb://127.0.0.1:27017".into(),
        "CHRONO_NYXID_SERVICE_ID" => "11111111-1111-4111-8111-111111111111".into(),
        _ => "placeholder".into(),
    }
}

/// Load staging's configuration without its graduated entries, plus
/// `overrides`.
///
/// Loading reads the control key file, so it is a temporary file holding a
/// key of valid length. The CA path is only checked to be absolute and is
/// read later by the Kubernetes client, so it keeps staging's live path.
fn staging(overrides: &[(&str, &str)]) -> Result<Config, String> {
    let control_key = Mounted::new(&format!("{}\n", "x".repeat(32)));
    let mut values: HashMap<String, String> = STAGING
        .iter()
        .filter(|(key, _)| !GRADUATED.contains(key))
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    for name in STAGING_SECRETS {
        values.insert((*name).to_owned(), placeholder(name));
    }
    values.insert(CONTROL_KEY_FILE.to_owned(), control_key.path());
    for (key, value) in overrides {
        values.insert((*key).to_owned(), (*value).to_owned());
    }
    Config::from_lookup(|key| Ok(values.get(key).cloned()))
}

/// Every graduated gate as the loaded configuration holds it.
fn observed(config: &Config) -> Vec<(&'static str, bool)> {
    let flags = &config.features;
    let sandbox = config
        .runtime
        .agent_sandbox
        .as_ref()
        .expect("staging allocates native sandboxes");
    vec![
        (
            "CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED",
            flags.agent_control_write_coordination,
        ),
        (
            "CHRONO_COMMAND_RECONCILE_PACING_ENABLED",
            flags.command_reconcile_pacing,
        ),
        (
            "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED",
            flags.ownership_cleanup_pacing,
        ),
        (
            "CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED",
            sandbox.native_claim_guard_enabled,
        ),
        ("CHRONO_V2_UI_TRANSPORT_ENABLED", flags.v2_ui_transport),
    ]
}

#[test]
fn every_graduated_gate_is_on_when_unset_and_off_when_explicitly_false() {
    assert_eq!(
        GRADUATED,
        [
            "CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED",
            "CHRONO_COMMAND_RECONCILE_PACING_ENABLED",
            "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED",
            "CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED",
            "CHRONO_V2_UI_TRANSPORT_ENABLED",
        ]
    );
    for name in GRADUATED {
        let read = |value: Option<&str>| {
            features::flag(
                &mut |key| Ok((key == *name).then(|| value.map(str::to_owned)).flatten()),
                name,
            )
        };
        assert!(read(None).unwrap(), "{name} unset");
        assert!(read(Some("true")).unwrap(), "{name}=true");
        assert!(!read(Some("false")).unwrap(), "{name}=false");
        for value in ["", "1", "TRUE", " true ", "false "] {
            assert_eq!(
                read(Some(value)).unwrap_err(),
                format!("{name} must be true or false")
            );
        }
    }
    // An ungraduated gate keeps its off default.
    assert!(!features::flag(&mut |_| Ok(None), "CHRONO_NYXID_PROXY_ENABLED").unwrap());
}

#[test]
fn staging_loads_with_graduated_gates_unset() {
    let config = staging(&[]).expect("staging loads");
    let observed = observed(&config);
    assert_eq!(
        observed.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        GRADUATED
    );
    for (name, enabled) in observed {
        assert!(enabled, "{name} unset");
    }
}

#[test]
fn staging_loads_with_every_graduated_gate_explicitly_false() {
    let off: Vec<_> = GRADUATED.iter().map(|name| (*name, "false")).collect();
    let config = staging(&off).expect("staging loads");
    for (name, enabled) in observed(&config) {
        assert!(!enabled, "{name}=false");
    }
}

#[test]
fn staging_loads_with_each_graduated_gate_alone_explicitly_false() {
    for off in GRADUATED {
        // A config error may echo fixture secrets, so only the gate is named.
        let config = staging(&[(off, "false")]).unwrap_or_else(|_| panic!("{off}=false refused"));
        for (name, enabled) in observed(&config) {
            assert_eq!(enabled, name != *off, "{name} with {off}=false");
        }
    }
}

#[test]
fn the_staging_fixture_matches_live_keys_and_file_handling() {
    assert_eq!(STAGING.len(), 51);
    let live = |key: &str| {
        STAGING
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| *value)
            .unwrap()
    };
    assert_eq!(live(CA_FILE), "/etc/cma-staging/sandbox-kubernetes-ca.crt");
    assert_eq!(
        live(CONTROL_KEY_FILE),
        "/etc/cma-staging/agent-sandbox-control-key"
    );
    let config = staging(&[]).expect("staging loads");
    let connection = config
        .runtime
        .agent_sandbox_connection
        .as_ref()
        .expect("staging connects to its sandbox cluster");
    assert_eq!(connection.ca_file, live(CA_FILE));
    // The control key is read at load time: a short or absent key is refused.
    let short = Mounted::new("short\n");
    assert!(staging(&[(CONTROL_KEY_FILE, &short.path())]).is_err());
    let absent = std::env::temp_dir().join(format!("cma-graduation-{}", uuid::Uuid::new_v4()));
    assert!(staging(&[(CONTROL_KEY_FILE, &absent.to_string_lossy())]).is_err());
}
