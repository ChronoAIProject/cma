//! Explicit setup inputs. Issuer, provider origin, callback and custody are server-owned.
use serde::{Deserialize, Serialize};

string_enum! { /// Platform of the selected existing bot.
    pub enum BridgePlatform { Telegram => "telegram", Discord => "discord", Lark => "lark", Feishu => "feishu" }
}
string_enum! { /// Explicit model-context partition policy.
    pub enum BridgePartitionPolicy {
        ConversationSender => "conversation_sender", Conversation => "conversation",
        ThreadSender => "thread_sender", Thread => "thread", FixedSharedAgent => "fixed_shared_agent",
        SenderAcrossChats => "sender_across_chats"
    }
}
string_enum! { /// Treatment of work accepted under an earlier binding.
    pub enum BridgeAcceptedWork { DrainOriginal => "drain_original", SuspendOriginal => "suspend_original" }
}
string_enum! { /// Explicit Agent reply policy. Legacy `final_response` wire spelling
    /// allows tool-only delivery during a turn by channel-created Agents; final
    /// answers are not automatically sent. Approval controls remain unavailable.
    pub enum BridgeReplyPolicy { FinalResponse => "final_response", FinalResponseAndApprovals => "final_response_and_approvals", Disabled => "disabled" }
}
string_enum! { /// Required verified addressing evidence for group messages.
    pub enum BridgeGroupAdmission { Deny => "deny", All => "all", Mention => "mention", ReplyToBot => "reply_to_bot", MentionOrReplyToBot => "mention_or_reply_to_bot" }
}

/// Exact selected published profile revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeProfileRef {
    /// Profile identifier.
    pub id: String,
    /// Positive immutable published revision.
    pub revision: u32,
}
/// Exact retained Agent ancestry, without a grant or ambient owner authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeAgentTarget {
    /// Retained Sandbox.
    pub sandbox_id: String,
    /// Retained Workspace.
    pub workspace_id: String,
    /// Retained Worktree.
    pub worktree_id: String,
    /// Retained Agent.
    pub agent_id: String,
}
/// Existing shared context or new resources from three explicit clean templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgePlacement {
    /// Reuse one retained Agent, requiring explicit shared context and separate invoke authority.
    ExistingAgent {
        /// Exact selected target ancestry.
        target: BridgeAgentTarget,
    },
    /// Select one exact published Sandbox, Workspace and Agent profile.
    NewSandbox {
        /// Sandbox template.
        sandbox_profile: BridgeProfileRef,
        /// Workspace template.
        workspace_profile: BridgeProfileRef,
        /// Agent template.
        agent_profile: BridgeProfileRef,
    },
}
/// Explicit opaque sender or chat identity gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeIdentityAdmission {
    /// Admit no identity through this gate.
    Deny,
    /// Admit any identity satisfying all other verified gates.
    Open,
    /// Admit only the selected exact identifiers.
    Allowlist {
        /// Unique opaque IDs, at most 512; display names are not identities.
        ids: Vec<String>,
    },
}
/// Sender and chat gates intersect with verified group addressing policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeScopedAdmission {
    /// Exact sender selection.
    pub senders: BridgeIdentityAdmission,
    /// Exact actual-chat selection.
    pub chats: BridgeIdentityAdmission,
    /// Group/channel addressing rule.
    pub groups: BridgeGroupAdmission,
}
/// Admission policy; an open gate never bypasses bot/system/loop exclusions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeAdmission {
    /// Retained open policy.
    All,
    /// Retained sender allowlist syntax.
    Allowlist {
        /// Unique selected sender identities.
        sender_ids: Vec<String>,
    },
    /// Explicit sender, actual-chat and group policies.
    Scoped {
        /// Intersected admission gates.
        policy: BridgeScopedAdmission,
    },
}
/// Select existing provider objects. No field can override deployment routing or custody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeBinding {
    /// Existing selected bot.
    pub bot_id: String,
    /// Existing selected bot platform.
    pub platform: BridgePlatform,
    /// Unique selected route identifiers, one through 64.
    pub route_ids: Vec<String>,
    /// Existing selected key identifier, never its secret value.
    pub key_id: String,
}
/// Positive bounded admission and retention limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeLimits {
    /// At most 100,000 retained partitions.
    pub max_partitions: u32,
    /// At most the selected partition bound.
    pub max_sandboxes: u32,
    /// At most 100,000 queued messages.
    pub max_queued_messages: u32,
    /// At most 100,000 accepted messages per minute.
    pub max_messages_per_minute: u32,
    /// At most 1 MiB of received payload bytes.
    pub max_payload_bytes: u32,
    /// At most 30 days of inbox retention.
    pub inbox_retention_seconds: u32,
    /// At most the selected inbox retention.
    pub reply_secret_retention_seconds: u32,
}
/// Configuration accepted by setup, excluding server-derived connection fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfiguration {
    /// Display name, at most 200 UTF-8 bytes without controls.
    pub display_name: String,
    /// Existing bot/key/route selection.
    pub binding: BridgeBinding,
    /// Explicit context-sharing policy.
    pub partition: BridgePartitionPolicy,
    /// Exact retained target or immutable templates.
    pub placement: BridgePlacement,
    /// Verified sender/chat admission gates.
    pub admission: BridgeAdmission,
    /// Outbound final-output policy.
    pub reply: BridgeReplyPolicy,
    /// Bounded capacity and retention.
    pub limits: BridgeLimits,
}
