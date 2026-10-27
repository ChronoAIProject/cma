//! Safe common event vocabulary, separate from the v1 raw-log projection.
use crate::agents::{
    AgentOperationDelivery, AgentOperationKind, AgentSkillInstallStatus, AgentWorkStep,
    FirstMessageDelivery,
};
use crate::instances::AgentStatus;
use serde::Deserialize;

string_enum! { /// Safe message author role.
    pub enum AgentMessageRole { User => "user", Assistant => "assistant" }
}
/// Safe failure summary without a runtime locator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedFailure {
    /// Public failure class.
    pub class: String,
    /// Public explanatory detail.
    pub detail: String,
}
/// One durable plan entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPlanStep {
    /// Display title.
    pub title: String,
    /// Original projected status.
    pub status: String,
}
/// Safe configuration option descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigOption {
    /// Option identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Option kind.
    pub kind: String,
}
/// Safe advertised runtime command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAvailableCommand {
    /// Command name.
    pub name: String,
    /// Public description.
    pub description: String,
}
/// Safe durable event; live reasoning never enters this vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEventKind {
    /// Safe durable created record.
    Created {
        /// The public `workspace_id` observation from this record.
        workspace_id: String,
        /// The public `worktree_id` observation from this record.
        worktree_id: String,
    },
    /// Safe durable conversation available record.
    ConversationAvailable,
    /// Safe durable runtime changed record.
    RuntimeChanged,
    /// Safe durable work completed record.
    WorkCompleted {
        /// The public `step` observation from this record.
        step: AgentWorkStep,
    },
    /// Safe durable work failed record.
    WorkFailed {
        /// The public `step` observation from this record.
        step: AgentWorkStep,
        /// The public `failure` observation from this record.
        failure: TypedFailure,
    },
    /// Runtime-confirmed state of an additive Agent skill.
    SkillInstallationChanged {
        /// Owner-visible skill identity.
        skill_id: String,
        /// Pending, confirmed installed, or failed installation.
        status: AgentSkillInstallStatus,
    },
    /// Safe durable turn started record.
    TurnStarted {
        /// The public `turn_sequence` observation from this record.
        turn_sequence: u64,
    },
    /// Safe durable turn metadata record.
    TurnMetadata {
        /// The public `turn_sequence` observation from this record.
        turn_sequence: u64,
        /// The public `metadata` observation from this record.
        metadata: std::collections::BTreeMap<String, String>,
    },
    /// Safe durable message record.
    Message {
        /// The public `role` observation from this record.
        role: AgentMessageRole,
        /// The public `content` observation from this record.
        content: String,
    },
    /// Safe durable turn completed record.
    TurnCompleted {
        /// The public `turn_sequence` observation from this record.
        turn_sequence: u64,
    },
    /// Safe durable turn failed record.
    TurnFailed {
        /// The public `turn_sequence` observation from this record.
        turn_sequence: u64,
        /// The public `failure` observation from this record.
        failure: TypedFailure,
    },
    /// Safe durable turn cancelled record.
    TurnCancelled {
        /// The public `turn_sequence` observation from this record.
        turn_sequence: u64,
    },
    /// Safe durable tool started record.
    ToolStarted {
        /// The public `call_id` observation from this record.
        call_id: String,
        /// The public `title` observation from this record.
        title: String,
        /// The public `kind` observation from this record.
        kind: String,
        /// The public `status` observation from this record.
        status: String,
    },
    /// Safe durable tool updated record.
    ToolUpdated {
        /// The public `call_id` observation from this record.
        call_id: String,
        /// The public `status` observation from this record.
        status: String,
        /// The public `title` observation from this record.
        #[serde(deserialize_with = "super::types::required_nullable")]
        title: Option<String>,
    },
    /// Safe durable approval requested record.
    ApprovalRequested {
        /// The public `approval_id` observation from this record.
        approval_id: String,
        /// The public `action` observation from this record.
        #[serde(deserialize_with = "super::types::required_nullable")]
        action: Option<String>,
        /// The public `detail` observation from this record.
        #[serde(deserialize_with = "super::types::required_nullable")]
        detail: Option<String>,
    },
    /// Safe durable approval resolved record.
    ApprovalResolved {
        /// The public `approval_id` observation from this record.
        approval_id: String,
        /// The public `decision` observation from this record.
        decision: String,
    },
    /// Safe durable delivery changed record.
    DeliveryChanged {
        /// The public `operation_sequence` observation from this record.
        operation_sequence: u64,
        /// The public `operation` observation from this record.
        operation: AgentOperationKind,
        /// The public `state` observation from this record.
        state: AgentOperationDelivery,
    },
    /// Safe durable usage record.
    Usage {
        /// The public `input_tokens` observation from this record.
        input_tokens: u64,
        /// The public `output_tokens` observation from this record.
        output_tokens: u64,
        /// The public `context_used` observation from this record.
        context_used: u64,
        /// The public `context_window` observation from this record.
        #[serde(deserialize_with = "super::types::required_nullable")]
        context_window: Option<u64>,
    },
    /// Updated canonical provider metadata, available to personal owners.
    RuntimeMetadataChanged {
        /// Current metadata snapshot.
        metadata: crate::agents::AgentRuntimeMetadata,
    },
    /// Actual lifetime and per-turn consumed usage, independent of context size.
    UsageUpdated {
        /// Lifetime input, including cached input.
        input_tokens: u64,
        /// Lifetime output.
        output_tokens: u64,
        /// Current context observation.
        context_used: u64,
        /// Known context limit.
        context_window: Option<u64>,
        /// Most recently observed durable turn.
        turn_sequence: Option<u64>,
        /// Consumed input in that turn.
        turn_input_tokens: u64,
        /// Consumed output in that turn.
        turn_output_tokens: u64,
    },
    /// Safe durable mode changed record.
    ModeChanged {
        /// The public `mode` observation from this record.
        mode: String,
    },
    /// Safe durable plan set record.
    PlanSet {
        /// The public `entries` observation from this record.
        entries: Vec<AgentPlanStep>,
    },
    /// Safe durable config options set record.
    ConfigOptionsSet {
        /// The public `options` observation from this record.
        options: Vec<AgentConfigOption>,
    },
    /// Safe durable available commands set record.
    AvailableCommandsSet {
        /// The public `commands` observation from this record.
        commands: Vec<AgentAvailableCommand>,
    },
    /// Safe durable history gap record.
    HistoryGap {
        /// The public `frames` observation from this record.
        frames: u64,
    },
    /// Safe durable authentication required record.
    AuthenticationRequired,
    /// Safe durable status changed record.
    StatusChanged {
        /// The public `from` observation from this record.
        from: AgentStatus,
        /// The public `to` observation from this record.
        to: AgentStatus,
    },
    /// Safe durable first message changed record.
    FirstMessageChanged {
        /// The public `state` observation from this record.
        state: FirstMessageDelivery,
    },
    /// Safe durable title set record.
    TitleSet {
        /// The public `title` observation from this record.
        title: String,
        /// The public `source` observation from this record.
        source: String,
    },
    /// Safe durable stopped record.
    Stopped {
        /// The public `reason` observation from this record.
        reason: String,
    },
    /// Safe durable deleted record.
    Deleted,
}
