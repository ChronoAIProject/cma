use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_HOME_DIRECTORY: &str = "/home/cma";
pub const DEFAULT_SUPERVISOR_PROGRAM: &str = "/usr/local/bin/layer4-runtime";
pub const DEFAULT_COMMAND_TIMEOUT_SECONDS: u64 = 600;
pub const DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES: u32 = 32 * 1024;

/// Provider-owned opaque runtime address.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeHandle(pub String);

impl RuntimeHandle {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub reaches: Vec<String>,
    pub own_credential: bool,
    pub home_directory: String,
    pub supervisor_program: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            sandbox_id: String::new(),
            capacity: SandboxCapacity::default(),
            environment: BTreeMap::new(),
            reaches: Vec::new(),
            own_credential: false,
            home_directory: DEFAULT_HOME_DIRECTORY.to_string(),
            supervisor_program: DEFAULT_SUPERVISOR_PROGRAM.to_string(),
            timeout_seconds: None,
            created_by: None,
        }
    }
}

/// One bounded sandbox-wide installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(rename = "timeout_s")]
    pub timeout_seconds: u64,
    #[serde(rename = "output_limit")]
    pub output_limit_bytes: u32,
}

impl Default for InstallRequest {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            timeout_seconds: DEFAULT_COMMAND_TIMEOUT_SECONDS,
            output_limit_bytes: DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        }
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
    #[serde(default)]
    pub commands: Vec<CommandReceipt>,
}

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
