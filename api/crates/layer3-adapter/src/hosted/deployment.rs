use std::collections::BTreeMap;

use crate::provider::{ProviderImage, ProviderSandboxRequest};
use crate::{
    AdapterCapabilities, AdapterError, RuntimeContract, RuntimeImage, SandboxSpec,
    RUNTIME_CONTRACT_LABEL, RUNTIME_CONTRACT_VERSION,
};

pub const PLACEHOLDER_CREDENTIAL: &str = "local-proxy-no-credential-needed";
pub const MODEL_PROXY_BASE_URL: &str = "http://127.0.0.1:9091";
pub const NYXID_PROXY_BASE_URL: &str = "http://127.0.0.2:9092";
const MODEL_PROXY_BIND: &str = "127.0.0.1:9091";
const NYXID_PROXY_BIND: &str = "127.0.0.2:9092";

/// Deployment-owned hosted runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedDeployment {
    pub image: RuntimeImage,
    pub upstream_base_url: String,
    pub upstream_wire_api: String,
    pub platform_base_url: String,
    pub default_timeout_seconds: Option<u64>,
}

impl Default for HostedDeployment {
    fn default() -> Self {
        Self {
            image: RuntimeImage {
                uri: "cma-runtime:dev".into(),
                labels: BTreeMap::from([(
                    RUNTIME_CONTRACT_LABEL.to_string(),
                    RUNTIME_CONTRACT_VERSION.to_string(),
                )]),
            },
            upstream_base_url: String::new(),
            upstream_wire_api: "responses".into(),
            platform_base_url: String::new(),
            default_timeout_seconds: None,
        }
    }
}

impl HostedDeployment {
    pub fn contract(&self) -> crate::Result<RuntimeContract> {
        RuntimeContract::admit(&self.image.uri, &self.image.labels)
    }

    pub fn capabilities(&self) -> AdapterCapabilities {
        self.contract()
            .map(RuntimeContract::capabilities)
            .unwrap_or_default()
    }

    /// Render one provider request without placing any real credential in it.
    pub fn render(&self, spec: &SandboxSpec) -> crate::Result<ProviderSandboxRequest> {
        self.contract()?;
        validate_spec(spec)?;

        let mut env = BTreeMap::from([
            ("HOME".to_string(), spec.home_directory.clone()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            (
                "CHRONO_RUNTIME_CONTRACT".to_string(),
                RUNTIME_CONTRACT_VERSION.to_string(),
            ),
            (
                "CHRONO_RUNTIME_HOME".to_string(),
                spec.home_directory.clone(),
            ),
            ("CHRONO_SANDBOX_ID".to_string(), spec.sandbox_id.clone()),
            ("CHRONO_SANDBOX_REACHES".to_string(), spec.reaches.join(",")),
            (
                "CHRONO_UPSTREAM_BASE_URL".to_string(),
                if spec.own_credential {
                    String::new()
                } else {
                    self.upstream_base_url.clone()
                },
            ),
            (
                "CHRONO_UPSTREAM_BIND".to_string(),
                MODEL_PROXY_BIND.to_string(),
            ),
            (
                "CHRONO_UPSTREAM_WIRE_API".to_string(),
                self.upstream_wire_api.clone(),
            ),
            (
                "CHRONO_PLATFORM_BASE_URL".to_string(),
                self.platform_base_url.clone(),
            ),
            (
                "CHRONO_PLATFORM_PROXY_BIND".to_string(),
                NYXID_PROXY_BIND.to_string(),
            ),
            ("NYXID_URL".to_string(), NYXID_PROXY_BASE_URL.to_string()),
            (
                "NYXID_ACCESS_TOKEN".to_string(),
                PLACEHOLDER_CREDENTIAL.to_string(),
            ),
        ]);
        for (name, value) in &spec.environment {
            if env.contains_key(name) || name.starts_with("CHRONO_") || name.starts_with("NYXID_") {
                return Err(AdapterError::Rejected(format!(
                    "sandbox environment may not override reserved variable {name}"
                )));
            }
            if name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0')
            {
                return Err(AdapterError::Rejected(format!(
                    "sandbox environment variable {name:?} is invalid"
                )));
            }
            env.insert(name.clone(), value.clone());
        }

        let mut metadata = BTreeMap::from([
            ("cma/sandbox".to_string(), spec.sandbox_id.clone()),
            (
                "cma/runtime-contract".to_string(),
                RUNTIME_CONTRACT_VERSION.to_string(),
            ),
        ]);
        if let Some(created_by) = &spec.created_by {
            metadata.insert("cma/control-plane".to_string(), created_by.clone());
        }

        Ok(ProviderSandboxRequest {
            image: ProviderImage {
                uri: self.image.uri.clone(),
            },
            entrypoint: vec![spec.supervisor_program.clone()],
            env,
            resource_limits: BTreeMap::from([
                ("cpu".to_string(), format!("{}m", spec.capacity.cpu_millis)),
                (
                    "memory".to_string(),
                    format!("{}Mi", spec.capacity.memory_mib),
                ),
            ]),
            timeout: spec.timeout_seconds.or(self.default_timeout_seconds),
            metadata,
        })
    }
}

fn validate_spec(spec: &SandboxSpec) -> crate::Result<()> {
    if spec.sandbox_id.trim().is_empty() {
        return Err(AdapterError::Rejected("sandbox id is empty".into()));
    }
    if spec.capacity.cpu_millis == 0 || spec.capacity.memory_mib == 0 {
        return Err(AdapterError::Rejected(
            "sandbox capacity must be greater than zero".into(),
        ));
    }
    if !spec.home_directory.starts_with('/')
        || !spec.supervisor_program.starts_with('/')
        || spec.home_directory.contains('\0')
        || spec.supervisor_program.contains('\0')
    {
        return Err(AdapterError::Rejected(
            "sandbox home and supervisor program must be absolute paths".into(),
        ));
    }
    Ok(())
}
