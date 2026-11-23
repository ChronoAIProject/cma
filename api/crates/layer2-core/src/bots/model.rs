//! Owner-managed external channels with durable resource and credential identities.
use crate::{
    profiles::{
        AgentProfileRef, AgentProfileRevision, SandboxProfileRef, SandboxProfileRevision,
        WorkspaceProfileRef, WorkspaceProfileRevision,
    },
    SubjectId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BotAdmission {
    All,
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        senders: IdentityAdmission,
        chats: IdentityAdmission,
        groups: GroupAdmission,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "ids",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum IdentityAdmission {
    Deny,
    Open,
    Allowlist(Vec<String>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupAdmission {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotReply {
    pub auto_final: bool,
    pub ack_after_ms: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotImageReference {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotRepository {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotInlineSkill {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BotInlinePermissions {
    #[serde(default)]
    pub services: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BotInlineAgentProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub skills: Vec<BotInlineSkill>,
    #[serde(default)]
    pub permissions: BotInlinePermissions,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<crate::event_workers::EventWorkerProfileRef>,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub external_bot_id: String,
    /// Opaque gateway-owned channel connection. Mutually exclusive with legacy external_bot_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_ref: Option<String>,
    /// Immutable Image Manager selection used by the inline consumer authoring path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<BotImageReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<BotRepository>,
    /// Inline Agent configuration. Profile references remain for historical Bot records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_agent: Option<BotInlineAgentProfile>,
    pub profile: AgentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_profile: Option<SandboxProfileRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_profile: Option<WorkspaceProfileRef>,
    pub admission: BotAdmission,
    pub reply: BotReply,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotProfiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_worker_profile: Option<crate::event_workers::EventWorkerProfilePin>,
    pub resolved_model: String,
    pub agent: AgentProfileRevision,
    pub sandbox: SandboxProfileRevision,
    pub workspace: WorkspaceProfileRevision,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotState {
    Creating,
    Active,
    Disabled,
    Failed { reason: String },
    Deleting,
    Deleted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BotOperation {
    Create,
    Disable,
    Enable,
    Rotate,
    Delete,
    Cleanup,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotWork {
    pub id: String,
    pub operation: BotOperation,
    pub step: String,
    pub generation: u64,
    pub holder: Option<String>,
    pub expires_at: Option<String>,
    /// Version one reserves every external mutation before sending it. Legacy
    /// claimed work has no proof that an expired request is terminal.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub effect_protocol: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<BotEffect>,
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// Custody of one possibly accepted mutation. A lease timeout never clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotEffect {
    pub id: String,
    pub step: String,
    pub deadline: String,
}
/// Metadata only. Credential bytes are never serializable through the Bot aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotCredentialSet {
    pub agent_key_id: Option<String>,
    pub agent_key_hash: Option<String>,
    pub route_key_id: Option<String>,
    pub route_key_hash: Option<String>,
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotChannel {
    pub channel_id: String,
    pub record_id: String,
    pub version: u64,
    pub binding_id: String,
    pub level: String,
    pub callback_url: String,
    pub event_url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotResources {
    pub channel: Option<BotChannel>,
    pub route_id: Option<String>,
    pub credentials: BotCredentialSet,
    pub replacement: Option<BotCredentialSet>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotTestResult {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: String,
    pub tested_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ownership: Option<super::BotOwnership>,
    pub bot_id: String,
    pub owner: SubjectId,
    pub request: CreateBot,
    pub profiles: BotProfiles,
    pub provider: String,
    pub record_id: String,
    pub resources: BotResources,
    pub state: BotState,
    pub work: Option<BotWork>,
    pub generation: u64,
    pub revision: u64,
    pub creation_key_hash: String,
    pub request_fingerprint: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_test: Option<BotTestResult>,
}
