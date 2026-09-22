#![allow(dead_code)]
use utoipa::ToSchema;

#[derive(ToSchema)]
pub(crate) struct PartitionIdsShape {
    sandbox_id: String,
    workspace_id: String,
    worktree_id: String,
    agent_id: String,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionResourceShape {
    Sandbox,
    Workspace,
    Worktree,
    Agent,
}
#[derive(ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PartitionTargetShape {
    Available {
        ids: PartitionIdsShape,
        #[schema(min_items = 1, max_items = 4)]
        present: Vec<PartitionResourceShape>,
    },
    Reserved {
        ids: PartitionIdsShape,
    },
    Redacted,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionPlatformShape {
    Telegram,
    #[serde(rename = "telegram-new")]
    TelegramNew,
    Discord,
    Lark,
    Feishu,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionConversationShape {
    Private,
    Group,
    Channel,
    Unknown,
}
#[derive(ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PartitionSourceShape {
    Available {
        platform: PartitionPlatformShape,
        chat_id: String,
        sender_id: String,
        sender_display_name: Option<String>,
        conversation_type: PartitionConversationShape,
    },
    Unavailable,
    Redacted,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionPlacementShape {
    ExistingAgent,
    NewSandbox,
    WorkspacePerPartition,
    WorktreeAgentPerPartition,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionContextShape {
    ChatSender,
    Chat,
    ThreadSender,
    Thread,
    SenderAcrossChats,
    FixedAgent,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionStateShape {
    Reserved,
    Provisioning,
    Ready,
    NeedsConnection,
    FailedRetryable,
    FailedTerminal,
    Retired,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgePartitionShape {
    id: String,
    bridge_id: String,
    #[schema(minimum = 1, maximum = 256)]
    generation: u64,
    revision: u64,
    config_revision: u64,
    current: bool,
    state: PartitionStateShape,
    placement: PartitionPlacementShape,
    context_sharing: PartitionContextShape,
    target: PartitionTargetShape,
    source: PartitionSourceShape,
    observed_at_ms: i64,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgePartitionPageShape {
    #[schema(max_items = 25)]
    items: Vec<BridgePartitionShape>,
    next_cursor: Option<String>,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionResponseStateShape {
    Queued,
    InProgress,
    WaitingAction,
    Completed,
    Failed,
    Cancelled,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionWaitShape {
    None,
    Worker,
    Approval,
    Action,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionDeliveryShape {
    DispatchFenced,
    WriteConfirmed,
    ExecutionObserved,
    PreWriteFailed,
    RecoveryRequired,
    RuntimeLost,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PartitionCommandShape {
    command_id: String,
    response_id: String,
    config_revision: u64,
    agent_order: u64,
    status: PartitionResponseStateShape,
    waiting_for: PartitionWaitShape,
    turn_sequence: Option<u64>,
    delivery: Option<PartitionDeliveryShape>,
    admitted_at: String,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PartitionAgentStateShape {
    Starting,
    Ready,
    Working,
    WaitingApproval,
    Paused,
    Stopped,
    Deleted,
    Failed,
}
#[derive(ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PartitionActivityShape {
    Available {
        agent_status: PartitionAgentStateShape,
        #[schema(max_items = 64)]
        pending: Vec<PartitionCommandShape>,
        pending_truncated: bool,
        source_unavailable: bool,
        latest: Option<PartitionCommandShape>,
        observed_at_ms: i64,
    },
    Unavailable,
    Redacted,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgePartitionDetailShape {
    partition: BridgePartitionShape,
    activity: PartitionActivityShape,
}

#[derive(ToSchema)]
pub(crate) struct PendingChannelMessageShape {
    command_id: String,
    response_id: String,
    agent_order: u64,
    admitted_at: String,
    platform: PartitionPlatformShape,
    #[schema(max_length = 1024)]
    text: String,
    text_truncated: bool,
}

#[derive(ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum PendingChannelMessagesShape {
    Available {
        bridge_id: String,
        partition_id: String,
        generation: u64,
        agent_id: String,
        through_sequence: u64,
        #[schema(max_items = 64)]
        messages: Vec<PendingChannelMessageShape>,
        pending_truncated: bool,
        source_unavailable: bool,
        observed_at_ms: i64,
    },
    Unavailable,
    Redacted,
}
