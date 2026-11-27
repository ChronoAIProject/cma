use std::collections::BTreeMap;

use crate::GithubRepositoryScope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_HOME_DIRECTORY: &str = "/home/cma";
pub const DEFAULT_SUPERVISOR_PROGRAM: &str = "/usr/local/bin/layer4-runtime";
pub const DEFAULT_COMMAND_TIMEOUT_SECONDS: u64 = 600;
pub const DEFAULT_TOTAL_TIMEOUT_SECONDS: u64 = 1_800;
pub const DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES: u32 = 32 * 1024;

/// Provider-owned opaque runtime address.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeHandle(pub String);

impl RuntimeHandle {
    /// Physical native member aliases for conservative startup scheduling only.
    /// This parsing grants no ownership; adoption must still prove the current object.
    pub fn native_member_identity(&self) -> crate::Result<Option<(&str, &str)>> {
        if self.0.starts_with("agent-sandbox:") {
            crate::agent_sandbox::parse_handle(self).map(Some)
        } else {
            Ok(None)
        }
    }
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Provider lifecycle state observed without assuming a closed provider enum.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLifecycle {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RuntimeLifecycle {
    pub fn is_running(&self) -> bool {
        self.state.eq_ignore_ascii_case("running")
    }
}

/// An image reference and the labels resolved from its manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeImage {
    pub uri: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// Numeric provider reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCapacity {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

impl Default for SandboxCapacity {
    fn default() -> Self {
        Self {
            cpu_millis: 1_000,
            memory_mib: 1_024,
        }
    }
}

/// Everything needed to provision one sandbox, excluding credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxSpec {
    pub sandbox_id: String,
    pub capacity: SandboxCapacity,
    /// An optional immutable image selected by Image Manager for this
    /// Sandbox.  `None` means the deployment-owned system default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_image: Option<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// Effective journal recovery on this retained runtime generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_journal_recovery: Option<bool>,
    #[serde(default)]
    pub reaches: Vec<String>,
    pub own_credential: bool,
    pub home_directory: String,
    pub supervisor_program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_volume: Option<Box<crate::HomeVolumeAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_proxy_url: Option<String>,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            sandbox_id: String::new(),
            capacity: SandboxCapacity::default(),
            runtime_image: None,
            environment: BTreeMap::new(),
            retained_journal_recovery: None,
            reaches: Vec::new(),
            own_credential: false,
            home_directory: DEFAULT_HOME_DIRECTORY.to_string(),
            supervisor_program: DEFAULT_SUPERVISOR_PROGRAM.to_string(),
            timeout_seconds: None,
            created_by: None,
            home_volume: None,
            tool_proxy_url: None,
        }
    }
}

/// Generation-specific provider provisioning request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProvisionRequest {
    /// Funding authority is persisted separately and injected for each effect;
    /// mutable coverage must not change an immutable provisioning receipt digest.
    #[serde(skip)]
    pub funded_until: Option<u64>,
    /// Verified, request-scoped descriptor for a canary's native home.
    #[serde(skip)]
    pub native_canary_image: Option<RuntimeImage>,
    pub generation: u64,
    pub spec: SandboxSpec,
    /// The release this operation was admitted against. It supplies the image
    /// only when the spec records none, so it never enters the receipt digest.
    #[serde(skip)]
    pub released: Option<crate::ReleasedRuntime>,
}

impl RuntimeProvisionRequest {
    pub fn new(generation: u64, spec: SandboxSpec) -> Self {
        Self {
            generation,
            spec,
            funded_until: None,
            native_canary_image: None,
            released: None,
        }
    }

    /// Render new work from the release captured at its admission.
    pub fn with_released(mut self, released: crate::ReleasedRuntime) -> Self {
        self.released = Some(released);
        self
    }
}

/// Exact provider runtime that a replacement control plane intends to adopt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeAdoptionRequest {
    pub generation: u64,
    pub handle: RuntimeHandle,
    pub spec: SandboxSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_image: Option<String>,
}

impl RuntimeAdoptionRequest {
    pub fn new(generation: u64, handle: RuntimeHandle, spec: SandboxSpec) -> Self {
        Self {
            generation,
            handle,
            spec,
            expected_image: None,
        }
    }

    /// Adopt the persisted image before rotating it to the current deployment.
    #[must_use]
    pub fn with_expected_image(mut self, image: impl Into<String>) -> Self {
        self.expected_image = Some(image.into());
        self
    }
}

/// Complete non-secret identity observed for one provider runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeObservation {
    pub sandbox_id: String,
    pub generation: u64,
    pub handle: RuntimeHandle,
    /// URL-safe base64 encoding of the SHA-256 image-reference digest.
    pub image_sha256: String,
    pub spec_fingerprint: String,
    pub runtime_contract: String,
    pub lifecycle: RuntimeLifecycle,
}

/// Provider and process observations collected within one adoption operation.
/// This contains observations, never reusable dispatch authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInspection {
    pub observation: RuntimeObservation,
    pub status: Option<crate::RuntimeStatus>,
}

/// One bounded sandbox-wide installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub operation_id: String,
    pub request_sha256: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(rename = "timeout_s")]
    pub timeout_seconds: u64,
    #[serde(rename = "total_timeout_s")]
    pub total_timeout_seconds: u64,
    #[serde(rename = "output_limit")]
    pub output_limit_bytes: u32,
}

impl Default for InstallRequest {
    fn default() -> Self {
        Self::bounded(
            String::new(),
            Vec::new(),
            DEFAULT_COMMAND_TIMEOUT_SECONDS,
            DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        )
    }
}

impl InstallRequest {
    pub fn new(operation_id: impl Into<String>, commands: Vec<String>) -> Self {
        Self::bounded(
            operation_id,
            commands,
            DEFAULT_COMMAND_TIMEOUT_SECONDS,
            DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        )
    }

    pub fn bounded(
        operation_id: impl Into<String>,
        commands: Vec<String>,
        timeout_seconds: u64,
        output_limit_bytes: u32,
    ) -> Self {
        Self::with_timeouts(
            operation_id,
            commands,
            timeout_seconds,
            DEFAULT_TOTAL_TIMEOUT_SECONDS,
            output_limit_bytes,
        )
    }

    pub fn with_timeouts(
        operation_id: impl Into<String>,
        commands: Vec<String>,
        timeout_seconds: u64,
        total_timeout_seconds: u64,
        output_limit_bytes: u32,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            request_sha256: String::new(),
            commands,
            timeout_seconds,
            total_timeout_seconds,
            output_limit_bytes,
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    /// Digest the exact execution-affecting fields with a versioned encoding.
    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.install.request.v2\0");
        digest.update(self.timeout_seconds.to_be_bytes());
        digest.update(self.total_timeout_seconds.to_be_bytes());
        digest.update(self.output_limit_bytes.to_be_bytes());
        digest.update(
            u32::try_from(self.commands.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        for command in &self.commands {
            digest.update(
                u32::try_from(command.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            digest.update(command.as_bytes());
        }
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Non-secret proof that one command completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandReceipt {
    pub index: u32,
    pub exit_code: i32,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
}

/// Proof that every requested install command completed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    #[serde(default)]
    pub commands: Vec<CommandReceipt>,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

/// Opaque credential bytes. Debug output is always redacted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeCredential(String);

impl RuntimeCredential {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RuntimeCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RuntimeCredential(<redacted>)")
    }
}

impl From<String> for RuntimeCredential {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for RuntimeCredential {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// One correlated sandbox credential installation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialFrame {
    pub operation_id: String,
    pub value: RuntimeCredential,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custody_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_route: Option<GithubCredentialRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubCredentialRoute {
    pub service_id: String,
    pub catalog_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_services: Vec<PlatformServiceRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_scope: Option<GithubRepositoryScope>,
}

impl GithubCredentialRoute {
    pub fn is_valid(&self) -> bool {
        valid_proxy_selector(&self.service_id)
            && valid_proxy_selector(&self.catalog_id)
            && self
                .repository_scope
                .as_ref()
                .is_none_or(|scope| scope.is_valid(&self.service_id))
            && self.other_services.len() <= 32
            && self.other_services.iter().all(|service| {
                valid_proxy_selector(&service.service_id)
                    && valid_proxy_selector(&service.slug)
                    && service.service_id != self.service_id
                    && service.slug != "api-github"
                    && service
                        .catalog_id
                        .as_ref()
                        .is_none_or(|id| valid_proxy_selector(id) && id != &self.catalog_id)
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformServiceRoute {
    pub service_id: String,
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
}

fn valid_proxy_selector(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

impl std::fmt::Debug for CredentialFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialFrame")
            .field("operation_id", &self.operation_id)
            .field("value", &self.value)
            .field("custody_generation", &self.custody_generation)
            .finish()
    }
}

/// Exact provider evidence from a terminated runtime incarnation, when available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTermination {
    pub out_of_memory: bool,
    pub finished_at: String,
}
