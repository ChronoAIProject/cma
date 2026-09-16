use super::super::{BridgeAgentTarget, BridgeCleanupAllocationState, BridgePlatform};
use crate::{agents::AgentOperationDelivery, instances::AgentStatus};
use serde::{Deserialize, Deserializer};

string_enum! { /// One exact retained target resource, in ancestry order.
    pub enum BridgePartitionResource { Sandbox => "sandbox", Workspace => "workspace", Worktree => "worktree", Agent => "agent" }
}
string_enum! { /// Which resources the partition creates or retains.
    pub enum BridgePartitionPlacement { ExistingAgent => "existing_agent", NewSandbox => "new_sandbox", WorkspacePerPartition => "workspace_per_partition", WorktreeAgentPerPartition => "worktree_agent_per_partition" }
}
string_enum! { /// External users intentionally sharing model context.
    pub enum BridgePartitionContext { ChatSender => "chat_sender", Chat => "chat", ThreadSender => "thread_sender", Thread => "thread", SenderAcrossChats => "sender_across_chats", FixedAgent => "fixed_agent" }
}
string_enum! { /// Normalized conversation classification from the accepted source.
    pub enum BridgePartitionConversation { Private => "private", Group => "group", Channel => "channel", Unknown => "unknown" }
}
string_enum! { /// Exact command status, independent of current local connectivity.
    pub enum BridgePartitionResponseStatus { Queued => "queued", InProgress => "in_progress", WaitingAction => "waiting_action", Completed => "completed", Failed => "failed", Cancelled => "cancelled" }
}
string_enum! { /// A held worker command is distinct from a pending approval.
    pub enum BridgePartitionWait { None => "none", Worker => "worker", Approval => "approval", Action => "action" }
}

/// Target links are omitted when current resource access cannot be established.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgePartitionTarget {
    /// At least one target resource exists; each existing resource was authorized.
    Available {
        /// Exact allocated ancestry, including resources still being created.
        ids: BridgeAgentTarget,
        /// Existing resources in Sandbox, Workspace, Worktree, Agent order.
        present: Vec<BridgePartitionResource>,
    },
    /// No target resource exists yet; these IDs are the reserved identities.
    Reserved {
        /// Exact reserved ancestry.
        ids: BridgeAgentTarget,
    },
    /// Target IDs and source/activity details are not available to this reader.
    Redacted {},
}

/// Facts from the first accepted source, not every sender in shared context.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgePartitionSource {
    /// Normalized historical evidence; all labels must be rendered as text.
    Available {
        /// Original platform.
        platform: BridgePlatform,
        /// Original actual platform chat, distinct from the route ID.
        chat_id: String,
        /// Original sender identity.
        sender_id: String,
        /// Optional bounded, plain-text display name.
        #[serde(deserialize_with = "required_nullable")]
        sender_display_name: Option<String>,
        /// Normalized conversation kind.
        conversation_type: BridgePartitionConversation,
    },
    /// Missing or expired original input with no qualifying retained command source.
    Unavailable {},
    /// Current target access does not permit revealing historical source labels.
    Redacted {},
}

/// Safe status for one exact partition allocation generation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartition {
    /// Exact allocation identity.
    pub id: String,
    /// Exact requested Bridge.
    pub bridge_id: String,
    /// Reset generation, from one through 256.
    pub generation: u64,
    /// Current allocation revision.
    pub revision: u64,
    /// Original pinned Bridge configuration revision.
    pub config_revision: u64,
    /// Whether this allocation is the current partition head.
    pub current: bool,
    /// Retained allocation state, including provisioning and connection needs.
    pub state: BridgeCleanupAllocationState,
    /// Exact placement mode.
    pub placement: BridgePartitionPlacement,
    /// Deliberate context-sharing boundary.
    pub context_sharing: BridgePartitionContext,
    /// Authorized ancestry or explicit redaction.
    pub target: BridgePartitionTarget,
    /// First accepted source, unavailable or redacted.
    pub source: BridgePartitionSource,
    /// Read observation in Unix milliseconds, not live connection health.
    pub observed_at_ms: u64,
}

/// Bounded page in ascending allocation-ID order, including retained generations.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionPage {
    /// At most the requested limit, never more than 25.
    pub items: Vec<BridgePartition>,
    /// Preserve unchanged and use only with this Bridge and operation.
    #[serde(deserialize_with = "required_nullable")]
    pub next_cursor: Option<String>,
}

/// Correlation with an exact Channel source; contains no input or grant secrets.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionCommand {
    /// Permanent command identity.
    pub command_id: String,
    /// Original response identity.
    pub response_id: String,
    /// This message's pinned Bridge configuration revision.
    pub config_revision: u64,
    /// Original command order within the exact Agent.
    pub agent_order: u64,
    /// Current retained command status.
    pub status: BridgePartitionResponseStatus,
    /// Worker hold, approval wait, other action, or no wait.
    pub waiting_for: BridgePartitionWait,
    /// Bound turn if dispatch began.
    #[serde(deserialize_with = "required_nullable")]
    pub turn_sequence: Option<u64>,
    /// Durable runtime-delivery observation, not an external platform reply.
    #[serde(deserialize_with = "required_nullable")]
    pub delivery: Option<AgentOperationDelivery>,
    /// Original admission timestamp.
    pub admitted_at: String,
}

/// Bounded command projection; retained attachment never proves local liveness.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgePartitionActivity {
    /// Accessible Agent with exact Channel command candidates.
    Available {
        /// Persisted Agent state.
        agent_status: AgentStatus,
        /// First 64 pending candidates, omitting any without retained source.
        pending: Vec<BridgePartitionCommand>,
        /// Additional pending candidates exist beyond the first 64.
        pending_truncated: bool,
        /// At least one bounded candidate lacks a qualifying retained source.
        source_unavailable: bool,
        /// Latest exact Channel command, if its source remains available.
        #[serde(deserialize_with = "required_nullable")]
        latest: Option<BridgePartitionCommand>,
        /// Snapshot time in Unix milliseconds.
        observed_at_ms: u64,
    },
    /// Target Agent is not present.
    Unavailable {},
    /// Target read authority could not be established.
    Redacted {},
}

/// Exact allocation summary with bounded Channel activity, separate from cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePartitionDetail {
    /// Requested allocation generation and first-source summary.
    pub partition: BridgePartition,
    /// Exact retained Channel command/response projection.
    pub activity: BridgePartitionActivity,
}

fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}
