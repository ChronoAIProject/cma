//! OpenAPI projections for core-owned public Bridge DTOs. Actual handlers
//! serialize the exact core projection; contract tests check the shared wire.
#![allow(dead_code)]
use super::super::resource_access_shapes::{ResourceActorView, ResourceOwnerView};
use layer2_core::{
    bridge::{setup::*, *},
    resource_access::ResourceOwner,
};
use serde::Deserialize;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateBridgeRequest {
    #[schema(value_type=ResourceOwnerView)]
    pub owner: ResourceOwner,
    #[schema(value_type=BridgeConfigShape)]
    pub config: BridgeSetupConfig,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateBridgeRequest {
    pub expected_revision: u64,
    #[schema(value_type=BridgeConfigShape)]
    pub config: BridgeSetupConfig,
    #[schema(value_type=Option<AcceptedWorkValue>)]
    pub previous_work: Option<AcceptedWorkPolicy>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlBridgeRequest {
    pub expected_revision: u64,
    #[schema(value_type=BridgeControlShape)]
    pub command: BridgeControlCommand,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeRevisionRequest {
    pub expected_revision: u64,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportBridgeKeyRequest {
    pub expected_revision: u64,
    /// Exact selected NyxID Agent key; write-only, never echoed or logged.
    #[schema(write_only, min_length = 1, max_length = 4096)]
    pub key: String,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevokeBridgeKeyRequest {
    pub expected_revision: u64,
    pub expected_custody_revision: u64,
}
#[derive(ToSchema)]
pub(crate) struct BridgeConfigShape {
    display_name: String,
    binding: BridgeBindingShape,
    partition: BridgePartitionValue,
    placement: BridgePlacementShape,
    admission: BridgeAdmissionShape,
    reply: BridgeReplyValue,
    limits: BridgeLimitsShape,
}
#[derive(ToSchema)]
pub(crate) struct BridgeBindingShape {
    bot_id: String,
    platform: BridgePlatformValue,
    route_ids: Vec<String>,
    key_id: String,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BridgePlatformValue {
    Telegram,
    Discord,
    Lark,
    Feishu,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BridgePartitionValue {
    Conversation,
    ConversationSender,
    Thread,
    ThreadSender,
    FixedSharedAgent,
    SenderAcrossChats,
}
#[derive(ToSchema)]
pub(crate) struct BridgeTargetShape {
    sandbox_id: String,
    workspace_id: String,
    worktree_id: String,
    agent_id: String,
}
#[derive(ToSchema)]
pub(crate) struct BridgeProfileShape {
    id: String,
    revision: u64,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BridgePlacementShape {
    ExistingAgent {
        target: BridgeTargetShape,
    },
    NewSandbox {
        sandbox_profile: BridgeProfileShape,
        workspace_profile: BridgeProfileShape,
        agent_profile: BridgeProfileShape,
    },
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BridgeAdmissionShape {
    All,
    Allowlist { sender_ids: Vec<String> },
    Scoped { policy: BridgeScopedAdmissionShape },
}
#[derive(ToSchema)]
pub(crate) struct BridgeScopedAdmissionShape {
    senders: BridgeIdentityAdmissionShape,
    chats: BridgeIdentityAdmissionShape,
    groups: BridgeGroupAdmissionValue,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BridgeIdentityAdmissionShape {
    Deny,
    Open,
    Allowlist { ids: Vec<String> },
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BridgeGroupAdmissionValue {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BridgeReplyValue {
    FinalResponse,
    FinalResponseAndApprovals,
    Disabled,
}
#[derive(ToSchema)]
pub(crate) struct BridgeLimitsShape {
    max_partitions: u32,
    max_sandboxes: u32,
    max_queued_messages: u32,
    max_messages_per_minute: u32,
    max_payload_bytes: u32,
    inbox_retention_seconds: u32,
    reply_secret_retention_seconds: u32,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AcceptedWorkValue {
    DrainOriginal,
    SuspendOriginal,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BridgeStateShape {
    Disabled { accepted_work: AcceptedWorkValue },
    Enabled,
    Retired { accepted_work: AcceptedWorkValue },
}
#[derive(ToSchema)]
pub(crate) struct BridgeEmergencyShape {
    ingress_stopped: bool,
    execution_stopped: bool,
    outbound_stopped: bool,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BridgeControlShape {
    Enable,
    Disable { accepted_work: AcceptedWorkValue },
    Retire { accepted_work: AcceptedWorkValue },
    Emergency { controls: BridgeEmergencyShape },
}
#[derive(ToSchema)]
pub(crate) struct BridgeViewShape {
    id: String,
    owner: ResourceOwnerView,
    display_name: String,
    revision: u64,
    config_revision: u64,
    state: BridgeStateShape,
    emergency: BridgeEmergencyShape,
    bot_id: String,
    platform: BridgePlatformValue,
    can_manage: bool,
    created_at: String,
    updated_at: String,
}
#[derive(ToSchema)]
pub(crate) struct BridgePageShape {
    items: Vec<BridgeViewShape>,
    next_cursor: Option<String>,
}
#[derive(ToSchema)]
pub(crate) struct BridgeReceiptShape {
    bridge_id: String,
    owner: ResourceOwnerView,
    actor: ResourceActorView,
    revision: u64,
    config_revision: u64,
    command: String,
    applied_at: String,
}
#[derive(ToSchema)]
pub(crate) struct BridgeCredentialStatusShape {
    revision: u64,
    present: bool,
    revoked: bool,
}
#[derive(ToSchema)]
pub(crate) struct BridgeConfigurationShape {
    bridge: BridgeViewShape,
    config: BridgeConfigShape,
    callback_url: String,
    credential: BridgeCredentialStatusShape,
}
#[derive(ToSchema)]
pub(crate) struct BridgeValidationShape {
    bridge_id: String,
    revision: u64,
    config_revision: u64,
    valid: bool,
}
#[derive(ToSchema)]
pub(crate) struct BridgeBotChoiceShape {
    id: String,
    platform: BridgePlatformValue,
    active: bool,
}
#[derive(ToSchema)]
pub(crate) struct BridgeKeyChoiceShape {
    id: String,
    active: bool,
    callback_configured: bool,
}
#[derive(ToSchema)]
pub(crate) struct BridgeRouteChoiceShape {
    id: String,
    bot_id: Option<String>,
    key_id: String,
    platform: String,
    active: bool,
}
#[derive(ToSchema)]
pub(crate) struct BridgeConnectorChoicesShape {
    bots: Vec<BridgeBotChoiceShape>,
    keys: Vec<BridgeKeyChoiceShape>,
    routes: Vec<BridgeRouteChoiceShape>,
}
