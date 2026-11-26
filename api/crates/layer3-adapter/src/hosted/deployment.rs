use std::collections::BTreeMap;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

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
pub(super) const SANDBOX_METADATA: &str = "cma/sandbox";
pub(super) const SPEC_FINGERPRINT_METADATA: &str = "cma/spec-sha256";
pub(super) const CONTROL_PLANE_METADATA: &str = "cma/control-plane";
pub(super) const RUNTIME_GENERATION_METADATA: &str = "cma/runtime-generation";
pub(super) const RUNTIME_IMAGE_METADATA: &str = "cma/image-sha256";
pub(super) const RUNTIME_CONTRACT_METADATA: &str = "cma/runtime-contract";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProviderIdentity {
    pub sandbox_id: String,
    pub generation: u64,
    pub image_sha256: String,
    pub spec_fingerprint: String,
    pub runtime_contract: String,
}

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
    pub(crate) fn render_adoption(
        &self,
        adoption: &crate::RuntimeAdoptionRequest,
        generation: u64,
    ) -> crate::Result<ProviderSandboxRequest> {
        let mut retained = self.clone();
        if let Some(image) = &adoption.expected_image {
            retained.image.uri.clone_from(image);
        }
        let request = retained.render_runtime(&adoption.spec, generation)?;
        if adoption
            .expected_image
            .as_ref()
            .is_some_and(|image| image != &request.image.uri)
        {
            return Err(AdapterError::Rejected(
                "persisted runtime image conflicts with its specification".into(),
            ));
        }
        Ok(request)
    }

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
        self.render_runtime(spec, 1)
    }

    /// Render one exact non-zero provider runtime generation.
    pub fn render_runtime(
        &self,
        spec: &SandboxSpec,
        generation: u64,
    ) -> crate::Result<ProviderSandboxRequest> {
        self.contract()?;
        validate_spec(spec)?;
        let image_uri = spec
            .runtime_image
            .as_deref()
            .unwrap_or(self.image.uri.as_str());
        if spec.runtime_image.is_some() {
            validate_runtime_image(image_uri)?;
        }
        if generation == 0 {
            return Err(AdapterError::Rejected(
                "runtime generation must be non-zero".into(),
            ));
        }

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
            (
                "CHRONO_RUNTIME_GENERATION".to_string(),
                generation.to_string(),
            ),
            ("CHRONO_SANDBOX_REACHES".to_string(), spec.reaches.join(",")),
            (
                "CHRONO_UPSTREAM_BASE_URL".to_string(),
                if spec.own_credential {
                    String::new()
                } else {
                    scoped_model_base_url(
                        &self.upstream_base_url,
                        &self.platform_base_url,
                        &spec.reaches,
                    )
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
        if spec.home_volume.is_some() {
            env.insert("TMPDIR".into(), super::home::TRANSFER_ROOT.into());
        }
        super::deployment_request::apply_retained_journal_policy(spec, &mut env)?;
        if let Some(endpoint) = &spec.tool_proxy_url {
            env.insert("CHRONO_TOOL_PROXY_URL".into(), endpoint.clone());
        }
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
            (SANDBOX_METADATA.to_string(), spec.sandbox_id.clone()),
            (
                RUNTIME_GENERATION_METADATA.to_string(),
                generation.to_string(),
            ),
            (RUNTIME_IMAGE_METADATA.to_string(), image_sha256(image_uri)),
            (
                RUNTIME_CONTRACT_METADATA.to_string(),
                RUNTIME_CONTRACT_VERSION.to_string(),
            ),
        ]);
        if let Some(created_by) = &spec.created_by {
            metadata.insert(CONTROL_PLANE_METADATA.to_string(), created_by.clone());
        }

        let mut request = ProviderSandboxRequest {
            funded_until: None,
            image: ProviderImage {
                uri: image_uri.to_owned(),
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
            volumes: Vec::new(),
        };
        super::home::render(&mut request, spec)?;
        request.metadata.insert(
            SPEC_FINGERPRINT_METADATA.to_string(),
            provider_request_fingerprint(&request)?,
        );
        Ok(request)
    }
}

pub(crate) fn scoped_model_base_url(
    configured: &str,
    platform: &str,
    reaches: &[String],
) -> String {
    let (Ok(mut upstream), Ok(platform)) = (
        reqwest::Url::parse(configured),
        reqwest::Url::parse(platform),
    ) else {
        return configured.to_owned();
    };
    if upstream.origin() != platform.origin()
        || !matches!(
            upstream.path(),
            "/api/v1/proxy/s/chrono-llm" | "/api/v1/proxy/s/chrono-llm-public"
        )
    {
        return configured.to_owned();
    }
    // Service scope is pinned with a Sandbox's credential. Older homes
    // must still revive with their original LLM route after the deployment
    // default switches to the public service. Prefer public for new scopes.
    let service = if reaches.iter().any(|service| service == "chrono-llm-public") {
        "chrono-llm-public"
    } else if reaches.iter().any(|service| service == "chrono-llm") {
        "chrono-llm"
    } else {
        return configured.to_owned();
    };
    upstream.set_path(&format!("/api/v1/proxy/s/{service}"));
    upstream.to_string()
}

pub(crate) fn validate_runtime_image(image: &str) -> crate::Result<()> {
    let Some((name, digest)) = image.rsplit_once("@sha256:") else {
        return Err(AdapterError::Rejected(
            "runtime image must be pinned by digest".into(),
        ));
    };
    if name.is_empty()
        || name.len() > 2_048
        || name.contains('@')
        || name.contains("://")
        || name.starts_with(['.', '-', '_', '/', ':'])
        || name.ends_with(['.', '-', '_', '/', ':'])
        || name.contains("//")
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(AdapterError::Rejected(
            "runtime image must be a valid immutable OCI reference".into(),
        ));
    }
    Ok(())
}

pub(crate) fn provider_request_fingerprint(
    request: &ProviderSandboxRequest,
) -> crate::Result<String> {
    request.validate_volumes()?;
    let ProviderSandboxRequest {
        funded_until: _,
        image,
        entrypoint,
        env,
        resource_limits,
        timeout,
        metadata,
        volumes,
    } = request;
    if metadata.contains_key(SPEC_FINGERPRINT_METADATA) {
        return Err(AdapterError::Rejected(
            "sandbox specification fingerprint is recursive".into(),
        ));
    }
    let mut digest = Sha256::new();
    digest.update(b"cma-provider-sandbox-request-v1\0");
    hash_string(&mut digest, &image.uri);
    hash_strings(&mut digest, entrypoint);
    hash_map(&mut digest, env);
    hash_map(&mut digest, resource_limits);
    match timeout {
        Some(timeout) => {
            digest.update([1]);
            digest.update(timeout.to_be_bytes());
        }
        None => digest.update([0]),
    }
    let stable_metadata = metadata
        .iter()
        .filter(|(name, _)| name.as_str() != CONTROL_PLANE_METADATA);
    hash_length(&mut digest, stable_metadata.clone().count());
    for (name, value) in stable_metadata {
        hash_string(&mut digest, name);
        hash_string(&mut digest, value);
    }
    // Existing volume-free runtime identities must survive this wire extension.
    // Bound volumes add a separate domain and cover every serialized mount field.
    if !volumes.is_empty() {
        digest.update(b"\0cma-provider-volumes-v1\0");
        let encoded = serde_json::to_string(volumes)
            .map_err(|error| AdapterError::Rejected(format!("encode runtime volumes: {error}")))?;
        hash_string(&mut digest, &encoded);
    }
    Ok(URL_SAFE_NO_PAD.encode(digest.finalize()))
}

pub(crate) fn provider_request_identity(
    request: &ProviderSandboxRequest,
) -> crate::Result<ProviderIdentity> {
    let identity = provider_metadata_identity(&request.metadata)?;
    if identity.image_sha256 != image_sha256(&request.image.uri) {
        return Err(AdapterError::Rejected(
            "provider request has an invalid image identity".into(),
        ));
    }
    Ok(identity)
}

pub(super) fn provider_metadata_identity(
    metadata: &BTreeMap<String, String>,
) -> crate::Result<ProviderIdentity> {
    let sandbox_id = required_metadata(metadata, SANDBOX_METADATA, "sandbox identity")?;
    if sandbox_id.trim() != sandbox_id
        || sandbox_id.len() > 256
        || sandbox_id.chars().any(char::is_control)
    {
        return Err(AdapterError::Rejected(
            "provider sandbox has an invalid sandbox identity".into(),
        ));
    }
    let spec_fingerprint = required_metadata(
        metadata,
        SPEC_FINGERPRINT_METADATA,
        "specification identity",
    )?;
    if spec_fingerprint.len() != 43
        || !spec_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AdapterError::Rejected(
            "provider sandbox has an invalid specification identity".into(),
        ));
    }
    let image = required_metadata(metadata, RUNTIME_IMAGE_METADATA, "image identity")?;
    if !sha256_fingerprint(image) {
        return Err(AdapterError::Rejected(
            "provider sandbox has an invalid image identity".into(),
        ));
    }
    let runtime_contract =
        required_metadata(metadata, RUNTIME_CONTRACT_METADATA, "runtime contract")?;
    if runtime_contract.trim() != runtime_contract
        || runtime_contract.len() > 64
        || runtime_contract.chars().any(char::is_control)
    {
        return Err(AdapterError::Rejected(
            "provider sandbox has an invalid runtime contract".into(),
        ));
    }
    Ok(ProviderIdentity {
        sandbox_id: sandbox_id.clone(),
        generation: required_generation(metadata)?,
        image_sha256: image.clone(),
        spec_fingerprint: spec_fingerprint.clone(),
        runtime_contract: runtime_contract.clone(),
    })
}

fn required_generation(metadata: &BTreeMap<String, String>) -> crate::Result<u64> {
    let value = required_metadata(metadata, RUNTIME_GENERATION_METADATA, "runtime generation")?;
    let generation = value.parse::<u64>().map_err(|_| {
        AdapterError::Rejected("provider runtime generation is not canonical".into())
    })?;
    if generation == 0 || generation.to_string() != *value {
        return Err(AdapterError::Rejected(
            "provider runtime generation is not canonical".into(),
        ));
    }
    Ok(generation)
}

fn required_metadata<'a>(
    metadata: &'a BTreeMap<String, String>,
    name: &str,
    description: &str,
) -> crate::Result<&'a String> {
    metadata
        .get(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AdapterError::Rejected(format!("provider sandbox has no {description}")))
}

fn sha256_fingerprint(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn image_sha256(uri: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(uri.as_bytes()))
}

fn hash_strings(digest: &mut Sha256, values: &[String]) {
    hash_length(digest, values.len());
    for value in values {
        hash_string(digest, value);
    }
}

fn hash_map(digest: &mut Sha256, values: &BTreeMap<String, String>) {
    hash_length(digest, values.len());
    for (name, value) in values {
        hash_string(digest, name);
        hash_string(digest, value);
    }
}

fn hash_string(digest: &mut Sha256, value: &str) {
    hash_length(digest, value.len());
    digest.update(value.as_bytes());
}

fn hash_length(digest: &mut Sha256, length: usize) {
    digest.update(u64::try_from(length).unwrap_or(u64::MAX).to_be_bytes());
}

fn validate_spec(spec: &SandboxSpec) -> crate::Result<()> {
    if spec.sandbox_id.trim().is_empty()
        || spec.sandbox_id.trim() != spec.sandbox_id
        || spec.sandbox_id.len() > 256
        || spec.sandbox_id.chars().any(char::is_control)
    {
        return Err(AdapterError::Rejected("sandbox id is invalid".into()));
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

#[cfg(test)]
#[path = "deployment_tests.rs"]
mod tests;
