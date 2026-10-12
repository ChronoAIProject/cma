//! Profile wire types for the independent CMAEG v1 protocol.
use super::{
    ActionMode, ApiType, ModelAccess, NyxIdIdentityKind, ProviderSlug, SkillSource, ThinkingEffort,
};
use serde::{Deserialize, Serialize};

/// CMAEG `EventWorkerProfile` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct EventWorkerProfile {
    /// Wire field `context`.
    pub context: ContextConfig,
    /// Wire field `environment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<serde_json::Value>,
    /// Wire field `metadata`.
    pub metadata: ProfileMetadata,
    /// Wire field `model`.
    pub model: ModelConfig,
    /// Wire field `permission`.
    pub permission: PermissionConfig,
    /// Wire field `schema_version`.
    pub schema_version: u64,
}

/// CMAEG `ContextConfig` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Wire field `mcp_servers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Vec<McpServerRef>>,
    /// Wire field `skills`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillRef>>,
    /// Wire field `system_prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// CMAEG `McpAuth` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpAuth {
    /// Wire variant `none`.
    #[serde(rename = "none")]
    None {},
    /// Wire variant `nyxid_service`.
    #[serde(rename = "nyxid_service")]
    NyxidService {
        /// Wire field `slug`.
        slug: String,
    },
}

/// CMAEG `McpServerRef` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerRef {
    /// Wire field `auth`.
    pub auth: McpAuth,
    /// Wire field `url`.
    pub url: String,
}

/// CMAEG `ModelConfig` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Wire field `access`.
    pub access: ModelAccess,
    /// Wire field `api_type`.
    pub api_type: ApiType,
    /// Wire field `model`.
    pub model: String,
    /// Wire field `service`.
    pub service: String,
    /// Wire field `thinking_effort`.
    pub thinking_effort: ThinkingEffort,
}

/// CMAEG `NyxIdIdentity` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct NyxIdIdentity {
    /// Wire field `kind`.
    pub kind: NyxIdIdentityKind,
    /// Wire field `subject`.
    pub subject: String,
}

/// CMAEG `PermissionConfig` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionConfig {
    /// Wire field `action_mode`.
    pub action_mode: ActionMode,
    /// Wire field `service_scope`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_scope: Option<Vec<String>>,
}

/// CMAEG `ProfileMetadata` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileMetadata {
    /// Wire field `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Wire field `name`.
    pub name: String,
    /// Wire field `owner`.
    pub owner: NyxIdIdentity,
    /// Wire field `provider`.
    pub provider: ProviderSlug,
}

/// CMAEG `SkillRef` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillRef {
    /// Wire field `id`.
    pub id: String,
    /// Wire field `source`.
    pub source: SkillSource,
    /// Wire field `version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// Compatibility alias for the original protocol 1 SDK name.
pub type AgentProfileSnapshot = EventWorkerProfile;
