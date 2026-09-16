use crate::{
    bridge::{
        allocation::AllocationState,
        partition::{ChannelConversationKind, PartitionContextSharing},
        BridgeAgentTarget, BridgeId, BridgePlatform,
    },
    ids::{AgentCommandId, AgentResponseId},
    sandbox::{agent_io::AgentResponseStatus, AgentOperationDelivery, AgentStatus},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionResource {
    Sandbox,
    Workspace,
    Worktree,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartitionTargetView {
    Available {
        ids: BridgeAgentTarget,
        present: Vec<PartitionResource>,
    },
    Reserved {
        ids: BridgeAgentTarget,
    },
    Redacted,
}

/// First accepted message facts, not an assertion that all senders/chats in a
/// deliberately shared partition have these same identities. Display as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartitionSourceView {
    Available {
        platform: BridgePlatform,
        chat_id: String,
        sender_id: String,
        sender_display_name: Option<String>,
        conversation_type: ChannelConversationKind,
    },
    Unavailable,
    Redacted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionPlacement {
    ExistingAgent,
    NewSandbox,
    WorkspacePerPartition,
    WorktreeAgentPerPartition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionView {
    pub id: String,
    pub bridge_id: BridgeId,
    pub generation: u64,
    pub revision: u64,
    pub config_revision: u64,
    pub current: bool,
    pub state: AllocationState,
    pub placement: PartitionPlacement,
    pub context_sharing: PartitionContextSharing,
    pub target: PartitionTargetView,
    pub source: PartitionSourceView,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionPage {
    pub items: Vec<BridgePartitionView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionCommandWait {
    None,
    Worker,
    Approval,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionCommandView {
    pub command_id: AgentCommandId,
    pub response_id: AgentResponseId,
    pub config_revision: u64,
    pub agent_order: u64,
    pub status: AgentResponseStatus,
    pub waiting_for: PartitionCommandWait,
    pub turn_sequence: Option<u64>,
    pub delivery: Option<AgentOperationDelivery>,
    pub admitted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PartitionActivityView {
    Available {
        /// Durable Agent state; does not prove a live local worker connection.
        agent_status: AgentStatus,
        pending: Vec<PartitionCommandView>,
        pending_truncated: bool,
        /// True when a bounded candidate has no retained normalized source.
        source_unavailable: bool,
        latest: Option<PartitionCommandView>,
        observed_at_ms: i64,
    },
    Unavailable,
    Redacted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionDetail {
    pub partition: BridgePartitionView,
    pub activity: PartitionActivityView,
}
