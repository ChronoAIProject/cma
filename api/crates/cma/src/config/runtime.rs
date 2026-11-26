use std::{fmt, fs};

use axum::http::HeaderValue;

use super::{parse_canonical_https_origin, required};

pub(crate) mod canary;

const MAX_URL_LENGTH: usize = 2_048;
const MAX_IMAGE_REFERENCE_LENGTH: usize = 2_048;
const SHA256_SEPARATOR: &str = "@sha256:";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Provider {
    OpenSandbox,
    AgentSandbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RuntimeMode {
    Execution(Provider),
    BackendFree,
}

impl RuntimeMode {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        match lookup("CHRONO_SANDBOX_PROVIDER")?.as_deref() {
            None | Some("opensandbox") => Ok(Self::Execution(Provider::OpenSandbox)),
            Some("agent-sandbox") => Ok(Self::Execution(Provider::AgentSandbox)),
            Some("none") => match lookup("CHRONO_ENVIRONMENT")?.as_deref() {
                Some("development" | "staging") => Ok(Self::BackendFree),
                _ => Err("CHRONO_SANDBOX_PROVIDER=none requires CHRONO_ENVIRONMENT=development or staging".into()),
            },
            Some(_) => Err("CHRONO_SANDBOX_PROVIDER must be opensandbox, agent-sandbox or none".into()),
        }
    }
}

/// Deployment-owned endpoints and runtime identity validated at startup.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RuntimeConfig {
    pub capacity: layer3_adapter::agent_sandbox::RuntimeCapacityConfig,
    pub migration: super::runtime_migration::RuntimeMigrationConfig,
    pub agent_sandbox: Option<layer3_adapter::agent_sandbox::AgentSandboxConfig>,
    pub agent_sandbox_connection: Option<layer3_adapter::agent_sandbox::RemoteKubernetesConfig>,
    pub agent_sandbox_runtime_transport: layer3_adapter::hosted::NativeRuntimeTransport,
    pub legacy_receiver_image: Option<layer3_adapter::hosted::LegacyReceiverImage>,
    pub stale_turn_runtime_digest: Option<String>,
    pub allocate_agent_sandbox: bool,
    pub native_prepared_target: u8,
    pub native_machine_prepared_target: u8,
    /// Number of unclaimed native agent-sandbox members kept ready for new Sandboxes.
    pub native_warm_pool_target: u32,
    /// Independent unclaimed capacity for the deployment desktop image.
    pub native_machine_warm_pool_target: u32,
    pub retain_opensandbox: bool,
    pub sandbox_base_url: String,
    pub sandbox_api_key: String,
    pub sandbox_image: String,
    pub github_repository_runtime_images: Vec<String>,
    /// Optional immutable image used when an Agent explicitly requests a
    /// desktop machine. Keeping this separate from the default image prevents
    /// ordinary Agent creation from silently changing its runtime.
    pub machine_sandbox_image: Option<String>,
    pub runtime_pointer: Option<crate::runtime_pointer::RuntimePointerConfig>,
    pub(crate) runtime_canary_releases: canary::Catalog,
    pub(crate) runtime_canary_writes: canary::WriteScope,
    pub(crate) retained_runtime_writes: canary::WriteScope,
    pub sandbox_namespace: String,
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub agent_sandbox_home_policy: Option<layer3_adapter::HomeVolumePolicy>,
    pub upstream_base_url: String,
    pub upstream_wire_api: String,
    pub platform_base_url: String,
}

impl fmt::Debug for RuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeConfig")
            .field("capacity", &self.capacity)
            .field("sandbox_base_url", &self.sandbox_base_url)
            .field("sandbox_api_key", &"<redacted>")
            .field("sandbox_image", &self.sandbox_image)
            .field("machine_sandbox_image", &self.machine_sandbox_image)
            .field(
                "runtime_canary_release_count",
                &self.runtime_canary_releases.len(),
            )
            .field("sandbox_namespace", &self.sandbox_namespace)
            .field("home_policy", &self.home_policy)
            .field("native_prepared_target", &self.native_prepared_target)
            .field(
                "native_machine_prepared_target",
                &self.native_machine_prepared_target,
            )
            .field("native_warm_pool_target", &self.native_warm_pool_target)
            .field(
                "native_machine_warm_pool_target",
                &self.native_machine_warm_pool_target,
            )
            .field("upstream_base_url", &self.upstream_base_url)
            .field("upstream_wire_api", &self.upstream_wire_api)
            .field("platform_base_url", &self.platform_base_url)
            .finish()
    }
}

impl RuntimeConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
        provider: Provider,
    ) -> Result<Self, String> {
        let allocate_agent_sandbox = provider == Provider::AgentSandbox;
        let retain_opensandbox = match lookup("CHRONO_RETAIN_OPENSANDBOX")?
            .as_deref()
            .unwrap_or("true")
        {
            "true" => true,
            "false" => false,
            _ => return Err("CHRONO_RETAIN_OPENSANDBOX must be true or false".into()),
        };
        let (sandbox_base_url, sandbox_api_key) = if retain_opensandbox || !allocate_agent_sandbox {
            let base = parse_canonical_http_origin(
                "CHRONO_SANDBOX_BASE_URL",
                &required(lookup, "CHRONO_SANDBOX_BASE_URL")?,
            )?;
            let key = read_api_key(&required(lookup, "CHRONO_SANDBOX_API_KEY_FILE")?)?;
            (base, key)
        } else {
            (String::new(), String::new())
        };
        let sandbox_image = parse_pinned_image(
            "CHRONO_SANDBOX_IMAGE",
            &required(lookup, "CHRONO_SANDBOX_IMAGE")?,
        )?;
        let machine_sandbox_image =
            match lookup("CHRONO_MACHINE_SANDBOX_IMAGE")?.filter(|value| !value.is_empty()) {
                Some(value) => Some(parse_pinned_image("CHRONO_MACHINE_SANDBOX_IMAGE", &value)?),
                None => None,
            };
        let sandbox_namespace = parse_sandbox_namespace(
            "CHRONO_SANDBOX_NAMESPACE",
            &required(lookup, "CHRONO_SANDBOX_NAMESPACE")?,
        )?;
        let upstream_base_url = parse_canonical_https_base_url(
            "CHRONO_UPSTREAM_BASE_URL",
            &required(lookup, "CHRONO_UPSTREAM_BASE_URL")?,
        )?;
        let upstream_wire_api = required(lookup, "CHRONO_UPSTREAM_WIRE_API")?;
        if upstream_wire_api != "responses" {
            return Err("CHRONO_UPSTREAM_WIRE_API must be \"responses\"".into());
        }
        let platform_base_url = parse_canonical_https_origin(
            "CHRONO_PLATFORM_BASE_URL",
            &required(lookup, "CHRONO_PLATFORM_BASE_URL")?,
        )?;
        let agent_sandbox =
            super::agent_sandbox::from_lookup(lookup, &sandbox_namespace, allocate_agent_sandbox)?;
        let agent_sandbox_runtime_transport = super::agent_sandbox::transport_from_lookup(lookup)?;
        let home_policy = super::home::from_lookup(lookup)?;
        let agent_sandbox_home_policy =
            super::home::native_from_lookup(lookup, home_policy.as_ref())?;
        let canary_home = agent_sandbox_home_policy
            .as_ref()
            .filter(|_| allocate_agent_sandbox && agent_sandbox.is_some());
        let runtime_canary_releases = canary::Catalog::from_lookup(lookup, canary_home)?;
        let runtime_canary_writes = canary::WriteScope::from_lookup(
            lookup,
            allocate_agent_sandbox,
            &runtime_canary_releases,
        )?;
        let retained_runtime_writes = canary::WriteScope::retained_from_lookup(
            lookup,
            allocate_agent_sandbox,
            &runtime_canary_releases,
        )?;
        let native_prepared_target = match lookup("CHRONO_NATIVE_PREPARED_TARGET")?.as_deref() {
            None | Some("0") => 0,
            Some("1") => 1,
            Some("2") => 2,
            _ => return Err("CHRONO_NATIVE_PREPARED_TARGET must be 0, 1 or 2".into()),
        };
        let native_machine_prepared_target =
            match lookup("CHRONO_NATIVE_MACHINE_PREPARED_TARGET")?.as_deref() {
                None | Some("0") => 0,
                Some("1") => 1,
                Some("2") => 2,
                _ => return Err("CHRONO_NATIVE_MACHINE_PREPARED_TARGET must be 0, 1 or 2".into()),
            };
        if native_machine_prepared_target > 0
            && (machine_sandbox_image.is_none()
                || !allocate_agent_sandbox
                || agent_sandbox.is_none()
                || !agent_sandbox_home_policy
                    .as_ref()
                    .is_some_and(|p| p.mounts_full_home()))
        {
            return Err("CHRONO_NATIVE_MACHINE_PREPARED_TARGET requires a machine image, native allocation and full native home".into());
        }
        let native_warm_pool_target = match lookup("CHRONO_NATIVE_WARM_POOL_TARGET")?.as_deref() {
            // Keep one native member ready whenever agent-sandbox is the selected
            // provider. Legacy OpenSandbox deployments remain unchanged.
            None if allocate_agent_sandbox => 1,
            None => 0,
            Some(value) => value
                .parse::<u32>()
                .ok()
                .filter(|target| *target <= 1000)
                .ok_or_else(|| {
                    "CHRONO_NATIVE_WARM_POOL_TARGET must be an integer from 0 to 1000".to_string()
                })?,
        };
        let native_machine_warm_pool_target =
            match lookup("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET")?.as_deref() {
                None if allocate_agent_sandbox && machine_sandbox_image.is_some() => 1,
                None => 0,
                Some(value) => value
                    .parse::<u32>()
                    .ok()
                    .filter(|target| *target <= 1000)
                    .ok_or_else(|| {
                        "CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET must be an integer from 0 to 1000"
                            .to_string()
                    })?,
            };
        if native_machine_warm_pool_target > 0
            && (machine_sandbox_image.is_none()
                || !allocate_agent_sandbox
                || agent_sandbox.is_none()
                || !agent_sandbox_home_policy
                    .as_ref()
                    .is_some_and(layer3_adapter::HomeVolumePolicy::is_native_vct))
        {
            return Err("CHRONO_NATIVE_MACHINE_WARM_POOL_TARGET requires a machine image, agent-sandbox allocation and a native VCT home".into());
        }
        if machine_sandbox_image.as_ref() == Some(&sandbox_image) {
            return Err(
                "CHRONO_MACHINE_SANDBOX_IMAGE must differ from the agent-only image".into(),
            );
        }
        if native_prepared_target > 0
            && (!allocate_agent_sandbox
                || agent_sandbox.is_none()
                || !agent_sandbox_home_policy
                    .as_ref()
                    .is_some_and(|policy| policy.mounts_full_home()))
        {
            return Err("CHRONO_NATIVE_PREPARED_TARGET requires agent-sandbox allocation and a full native home".into());
        }
        if native_warm_pool_target > 0
            && (!allocate_agent_sandbox
                || agent_sandbox.is_none()
                || !agent_sandbox_home_policy
                    .as_ref()
                    .is_some_and(layer3_adapter::HomeVolumePolicy::is_native_vct))
        {
            return Err("CHRONO_NATIVE_WARM_POOL_TARGET requires agent-sandbox allocation and a native VCT home".into());
        }
        // The deployment keeps the remote ConfigMap projected while native
        // allocation is disabled so it can be enabled without a manifest
        // rewrite. Do not validate or compose that optional remote target until
        // the native provider is explicitly enabled; otherwise the disabled
        // legacy OpenSandbox startup would require a remote token/CA it cannot
        // use.
        let agent_sandbox_connection = if agent_sandbox.is_some() {
            super::agent_sandbox::connection_from_lookup(lookup)?
        } else {
            None
        };
        if agent_sandbox.as_ref().is_some_and(|settings| {
            settings.namespace == sandbox_namespace
                && agent_sandbox_connection.is_none()
                && agent_sandbox_home_policy != home_policy
                && !agent_sandbox_home_policy
                    .as_ref()
                    .is_some_and(|policy| policy.is_native_vct())
        }) {
            return Err(concat!(
                "a distinct CHRONO_AGENT_SANDBOX_HOME policy requires an independent ",
                "CHRONO_AGENT_SANDBOX_NAMESPACE or CHRONO_AGENT_SANDBOX_API_SERVER"
            )
            .into());
        }

        let legacy_receiver_image = super::agent_sandbox::legacy_receiver_image(lookup)?;
        let stale_turn_runtime_digest = lookup("CHRONO_STALE_TURN_RUNTIME_DIGEST")?;
        if let Some(digest) = &stale_turn_runtime_digest {
            if !digest
                .strip_prefix("sha256:")
                .is_some_and(layer3_adapter::runtime_ledger_maintenance::valid_sha256)
            {
                return Err("CHRONO_STALE_TURN_RUNTIME_DIGEST must name a SHA-256 digest".into());
            }
        }
        let runtime_pointer = crate::runtime_pointer::RuntimePointerConfig::from_lookup(
            lookup,
            machine_sandbox_image.is_some(),
        )?;
        if runtime_pointer.is_some() && (!allocate_agent_sandbox || agent_sandbox.is_none()) {
            return Err("runtime pointer following requires native allocation".into());
        }
        if legacy_receiver_image.is_some()
            && (agent_sandbox.is_none()
                || agent_sandbox_runtime_transport
                    == layer3_adapter::hosted::NativeRuntimeTransport::Direct)
        {
            return Err(
                "CHRONO_LEGACY_RECEIVER_IMAGE_PROOF requires a Kubernetes-bound native transport"
                    .into(),
            );
        }
        let github_repository_runtime_images = lookup("CHRONO_GITHUB_REPOSITORY_RUNTIME_IMAGES")?
            .map(|images| {
                images
                    .split(',')
                    .map(|image| {
                        parse_pinned_image("CHRONO_GITHUB_REPOSITORY_RUNTIME_IMAGES", image)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self {
            capacity: super::runtime_capacity::from_lookup(lookup)?,
            migration: super::runtime_migration::RuntimeMigrationConfig::from_lookup(lookup)?,
            agent_sandbox,
            agent_sandbox_connection,
            agent_sandbox_runtime_transport,
            legacy_receiver_image,
            stale_turn_runtime_digest,
            allocate_agent_sandbox,
            native_prepared_target,
            native_machine_prepared_target,
            native_warm_pool_target,
            native_machine_warm_pool_target,
            retain_opensandbox,
            sandbox_base_url,
            sandbox_api_key,
            sandbox_image,
            github_repository_runtime_images,
            machine_sandbox_image,
            runtime_pointer,
            runtime_canary_releases,
            runtime_canary_writes,
            retained_runtime_writes,
            sandbox_namespace,
            home_policy,
            agent_sandbox_home_policy,
            upstream_base_url,
            upstream_wire_api,
            platform_base_url,
        })
    }
}

pub(crate) fn deployment_id_from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<String, String> {
    let value = required(lookup, "CHRONO_DEPLOYMENT_ID")?;
    let bytes = value.as_bytes();
    if !(1..=63).contains(&bytes.len())
        || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(concat!(
            "CHRONO_DEPLOYMENT_ID must be a Kubernetes label value: 1 to 63 ASCII ",
            "letters, digits, '.', '_' or '-', with an alphanumeric first and last character"
        )
        .into());
    }
    Ok(value)
}

fn parse_canonical_http_origin(key: &str, raw: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|_| format!("{key} must be one canonical absolute HTTP(S) origin"))?;
    let canonical = parsed.origin().ascii_serialization();
    if raw.len() > MAX_URL_LENGTH
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err(format!(
            "{key} must be one canonical absolute HTTP(S) origin"
        ));
    }
    Ok(canonical)
}

fn parse_canonical_https_base_url(key: &str, raw: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|_| format!("{key} must be one canonical absolute HTTPS base URL"))?;
    let origin = parsed.origin().ascii_serialization();
    let canonical = if parsed.path() == "/" {
        origin
    } else {
        format!("{origin}{}", parsed.path())
    };
    if raw.len() > MAX_URL_LENGTH
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw.ends_with('/')
        || raw != canonical
    {
        return Err(format!(
            "{key} must be one canonical absolute HTTPS base URL without a trailing slash"
        ));
    }
    Ok(canonical)
}

fn parse_pinned_image(key: &str, raw: &str) -> Result<String, String> {
    let valid = raw
        .rsplit_once(SHA256_SEPARATOR)
        .is_some_and(|(image, digest)| {
            raw.len() <= MAX_IMAGE_REFERENCE_LENGTH
                && !image.is_empty()
                && !image.contains('@')
                && !image.contains("://")
                && !image.starts_with(['.', '-', '_', '/', ':'])
                && !image.ends_with(['.', '-', '_', '/', ':'])
                && !image.contains("//")
                && image.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
                })
                && digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
    if !valid {
        return Err(format!(
            "{key} must be an immutable OCI image reference ending in @sha256: and 64 lowercase hexadecimal digits"
        ));
    }
    Ok(raw.to_owned())
}

fn parse_sandbox_namespace(key: &str, raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    if !(1..=63).contains(&bytes.len())
        || !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !bytes
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(format!(
            "{key} must be one lowercase DNS-1123 label of at most 63 bytes"
        ));
    }
    Ok(raw.to_owned())
}

fn read_api_key(path: &str) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!("CHRONO_SANDBOX_API_KEY_FILE {path:?} is not readable: {error}")
    })?;
    let contents = std::str::from_utf8(&bytes)
        .map_err(|_| format!("CHRONO_SANDBOX_API_KEY_FILE {path:?} must contain valid UTF-8"))?;
    let api_key = contents.trim();
    if api_key.is_empty() {
        return Err(format!(
            "CHRONO_SANDBOX_API_KEY_FILE {path:?} must contain a nonempty API key"
        ));
    }
    if HeaderValue::try_from(api_key).is_err() {
        return Err(format!(
            "CHRONO_SANDBOX_API_KEY_FILE {path:?} must contain one valid HTTP header value"
        ));
    }
    Ok(api_key.to_owned())
}
