//! Provider-neutral Event Worker Profiles.

use std::collections::BTreeMap;

use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::ToSchema;

use crate::{ids::ProviderSlug, require_string, ValidationError};

pub const PROFILE_SCHEMA_VERSION: u16 = 1;
pub const MAX_PROFILE_NAME_BYTES: usize = 80;
pub const MAX_PROFILE_DESCRIPTION_BYTES: usize = 2_000;
pub const MAX_SERVICE_SCOPE: usize = 64;
pub const MAX_SKILLS: usize = 32;
pub const MAX_SYSTEM_PROMPT_BYTES: usize = 32 * 1_024;
pub const MAX_MCP_SERVERS: usize = 8;
pub const MAX_ENVIRONMENT_BYTES: usize = 64 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NyxIdIdentityKind {
    Human,
    ServiceAccount,
    AgentKey,
    Organization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NyxIdIdentity {
    pub kind: NyxIdIdentityKind,
    pub subject: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelAccess {
    Managed,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiType {
    Responses,
    ChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingEffort {
    Minimal,
    Low,
    Medium,
    High,
    VeryHigh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub access: ModelAccess,
    pub service: String,
    pub api_type: ApiType,
    pub model: String,
    pub thinking_effort: ThinkingEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionMode {
    Guarded,
    Autonomous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionConfig {
    pub action_mode: ActionMode,
    #[serde(default)]
    pub service_scope: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Ornn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillRef {
    pub source: SkillSource,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum McpAuth {
    None {},
    NyxidService { slug: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerRef {
    pub url: String,
    pub auth: McpAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EventWorkerProfile {
    pub schema_version: u16,
    pub metadata: ProfileMetadata,
    pub model: ModelConfig,
    pub permission: PermissionConfig,
    pub context: ContextConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileMetadata {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub owner: NyxIdIdentity,
    pub provider: ProviderSlug,
}

impl EventWorkerProfile {
    /// Validate the snapshot before it is bound to a channel.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ValidationError::new(
                "/schema_version",
                "unsupported schema version",
            ));
        }
        require_string(
            &self.metadata.name,
            "/metadata/name",
            1,
            MAX_PROFILE_NAME_BYTES,
        )?;
        require_string(
            &self.metadata.description,
            "/metadata/description",
            0,
            MAX_PROFILE_DESCRIPTION_BYTES,
        )?;
        require_string(
            &self.metadata.owner.subject,
            "/metadata/owner/subject",
            1,
            512,
        )?;
        require_string(&self.model.service, "/model/service", 1, 512)?;
        require_string(&self.model.model, "/model/model", 1, 512)?;
        if self.permission.service_scope.len() > MAX_SERVICE_SCOPE {
            return Err(ValidationError::new(
                "/permission/service_scope",
                "too many services",
            ));
        }
        for (index, service) in self.permission.service_scope.iter().enumerate() {
            require_string(
                service,
                &format!("/permission/service_scope/{index}"),
                1,
                512,
            )?;
        }
        if self.context.skills.len() > MAX_SKILLS {
            return Err(ValidationError::new("/context/skills", "too many skills"));
        }
        for (index, skill) in self.context.skills.iter().enumerate() {
            require_string(&skill.id, &format!("/context/skills/{index}/id"), 1, 512)?;
            if let Some(version) = &skill.version {
                require_string(version, &format!("/context/skills/{index}/version"), 1, 128)?;
            }
        }
        require_string(
            &self.context.system_prompt,
            "/context/system_prompt",
            0,
            MAX_SYSTEM_PROMPT_BYTES,
        )?;
        if self.context.mcp_servers.len() > MAX_MCP_SERVERS {
            return Err(ValidationError::new(
                "/context/mcp_servers",
                "too many MCP servers",
            ));
        }
        for (index, server) in self.context.mcp_servers.iter().enumerate() {
            require_string(
                &server.url,
                &format!("/context/mcp_servers/{index}/url"),
                1,
                4_096,
            )?;
            if !server.url.starts_with("https://") {
                return Err(ValidationError::new(
                    format!("/context/mcp_servers/{index}/url"),
                    "must use https",
                ));
            }
        }
        if let Some(environment) = &self.environment {
            let bytes = serde_json::to_vec(environment).map_err(|error| {
                ValidationError::new("/environment", format!("cannot encode: {error}"))
            })?;
            if bytes.len() > MAX_ENVIRONMENT_BYTES {
                return Err(ValidationError::new(
                    "/environment",
                    "serialized value is too large",
                ));
            }
        }
        Ok(())
    }

    /// Hash the canonical profile JSON with SHA-256.
    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self).expect("profile snapshot is JSON serializable");
        let bytes = digest(&SHA256, canonical_json(value).as_bytes());
        format!("sha256:{}", hex(bytes.as_ref()))
    }
}

/// Serialize JSON with recursively sorted object keys and stable array order.
pub fn canonical_json(value: Value) -> String {
    fn sort(value: Value) -> Value {
        match value {
            Value::Object(object) => {
                let sorted = object
                    .into_iter()
                    .map(|(key, value)| (key, sort(value)))
                    .collect::<BTreeMap<_, _>>();
                Value::Object(sorted.into_iter().collect::<Map<_, _>>())
            }
            Value::Array(values) => Value::Array(values.into_iter().map(sort).collect()),
            other => other,
        }
    }
    serde_json::to_string(&sort(value)).expect("JSON values serialize infallibly")
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                HEX[(byte >> 4) as usize] as char,
                HEX[(byte & 0xf) as usize] as char,
            ]
        })
        .collect()
}

/// Compatibility name for clients compiled against the original v1 SDK.
pub type AgentProfileSnapshot = EventWorkerProfile;
