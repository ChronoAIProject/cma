use serde::{Deserialize, Serialize};

use crate::sandbox::{
    AgentAvailableCommand, AgentConfigOption, AgentLogEntry, AgentLogKind, AgentOperationDelivery,
    AgentOperationKind, AgentPlanStep, AgentStatus, AgentWorkStep, FirstMessageDelivery,
    TypedFailure,
};
use crate::AgentId;

pub const AGENT_IO_SCHEMA_VERSION: u16 = 1;

/// A replayable projection of the retained log. The original sequence is its
/// cursor even when an internal event is projected with fewer public fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentEvent {
    pub schema_version: u16,
    pub agent_id: AgentId,
    pub cursor: u64,
    pub appended_at: String,
    pub kind: AgentEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageRole {
    User,
    Assistant,
}

/// Public event data has no native conversation identity, approval locator,
/// credential, or hidden reasoning field. Safe tool summaries are retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentEventKind {
    Created {
        workspace_id: crate::WorkspaceId,
        worktree_id: crate::WorktreeId,
    },
    ConversationAvailable,
    RuntimeChanged,
    SkillInstallationChanged {
        skill_id: String,
        status: crate::sandbox::AgentSkillInstallStatus,
    },
    WorkCompleted {
        step: AgentWorkStep,
    },
    WorkFailed {
        step: AgentWorkStep,
        failure: TypedFailure,
    },
    TurnStarted {
        turn_sequence: u64,
    },
    TurnMetadata {
        turn_sequence: u64,
        metadata: std::collections::BTreeMap<String, String>,
    },
    Message {
        role: AgentMessageRole,
        content: String,
    },
    TurnCompleted {
        turn_sequence: u64,
    },
    TurnFailed {
        turn_sequence: u64,
        failure: TypedFailure,
    },
    TurnCancelled {
        turn_sequence: u64,
    },
    ToolStarted {
        call_id: String,
        title: String,
        kind: String,
        status: String,
    },
    ToolUpdated {
        call_id: String,
        status: String,
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<super::super::RecordedToolOutput>,
    },
    ToolOutputReported {
        call_id: String,
        output: super::super::RecordedToolOutput,
    },
    ApprovalRequested {
        approval_id: String,
        action: Option<String>,
        detail: Option<String>,
    },
    ApprovalResolved {
        approval_id: String,
        decision: String,
    },
    ExternalApprovalRequested {
        approval_id: String,
        provider: String,
        turn_sequence: u64,
        revision: u64,
        waiting_since: String,
        deadline_at: String,
        approve_url: Option<String>,
    },
    ExternalApprovalResolved {
        approval_id: String,
        provider: String,
        turn_sequence: u64,
        revision: u64,
        resolution: crate::sandbox::ExternalApprovalResolution,
    },
    DeliveryChanged {
        operation_sequence: u64,
        operation: AgentOperationKind,
        state: AgentOperationDelivery,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        context_window: Option<u64>,
    },
    RuntimeMetadataChanged {
        metadata: super::super::AgentRuntimeMetadata,
    },
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        context_window: Option<u64>,
        turn_sequence: Option<u64>,
        turn_input_tokens: u64,
        turn_output_tokens: u64,
    },
    ModeChanged {
        mode: String,
    },
    PlanSet {
        entries: Vec<AgentPlanStep>,
    },
    ConfigOptionsSet {
        options: Vec<AgentConfigOption>,
    },
    AvailableCommandsSet {
        commands: Vec<AgentAvailableCommand>,
    },
    HistoryGap {
        frames: u64,
    },
    AuthenticationRequired,
    StatusChanged {
        from: AgentStatus,
        to: AgentStatus,
    },
    FirstMessageChanged {
        state: FirstMessageDelivery,
    },
    TitleSet {
        title: String,
        source: String,
    },
    Stopped {
        reason: String,
    },
    Deleted,
}

impl From<AgentLogEntry> for AgentEvent {
    fn from(entry: AgentLogEntry) -> Self {
        let kind = match entry.kind {
            AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } => AgentEventKind::Created {
                workspace_id,
                worktree_id,
            },
            AgentLogKind::ConversationOpened { .. } => AgentEventKind::ConversationAvailable,
            AgentLogKind::RuntimeGenerationRetired { .. } => AgentEventKind::RuntimeChanged,
            AgentLogKind::SkillInstallationChanged { skill_id, status } => {
                AgentEventKind::SkillInstallationChanged { skill_id, status }
            }
            AgentLogKind::WorkStepCompleted { step } => AgentEventKind::WorkCompleted { step },
            AgentLogKind::WorkStepFailed { step, failure } => {
                AgentEventKind::WorkFailed { step, failure }
            }
            AgentLogKind::TurnStarted { turn_sequence, .. } => {
                AgentEventKind::TurnStarted { turn_sequence }
            }
            AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            } => AgentEventKind::TurnMetadata {
                turn_sequence,
                metadata,
            },
            AgentLogKind::OwnerMessage { content } => AgentEventKind::Message {
                role: AgentMessageRole::User,
                content,
            },
            AgentLogKind::AgentMessage { content } => AgentEventKind::Message {
                role: AgentMessageRole::Assistant,
                content,
            },
            AgentLogKind::TurnCompleted { turn_sequence } => {
                AgentEventKind::TurnCompleted { turn_sequence }
            }
            AgentLogKind::TurnFailed {
                turn_sequence,
                failure,
            } => AgentEventKind::TurnFailed {
                turn_sequence,
                failure,
            },
            AgentLogKind::TurnInterrupted { turn_sequence } => {
                AgentEventKind::TurnCancelled { turn_sequence }
            }
            AgentLogKind::ToolCallStarted {
                call_id,
                title,
                kind,
                status,
            } => AgentEventKind::ToolStarted {
                call_id,
                title,
                kind,
                status,
            },
            AgentLogKind::ToolCallUpdated {
                call_id,
                status,
                title,
                output,
            } => AgentEventKind::ToolUpdated {
                call_id,
                status,
                title,
                output,
            },
            AgentLogKind::ToolOutputReported { call_id, output } => {
                AgentEventKind::ToolOutputReported { call_id, output }
            }
            AgentLogKind::ApprovalRequested {
                approval_id,
                action,
                detail,
                ..
            } => AgentEventKind::ApprovalRequested {
                approval_id,
                action,
                detail,
            },
            AgentLogKind::ApprovalResolved {
                approval_id,
                decision,
            } => AgentEventKind::ApprovalResolved {
                approval_id,
                decision,
            },
            AgentLogKind::ExternalApprovalRequested {
                approval_id,
                provider,
                turn_sequence,
                revision,
                waiting_since,
                deadline_at,
            } => AgentEventKind::ExternalApprovalRequested {
                approval_id,
                provider,
                turn_sequence,
                revision,
                waiting_since,
                deadline_at,
                approve_url: None,
            },
            AgentLogKind::ExternalApprovalResolved {
                approval_id,
                provider,
                turn_sequence,
                revision,
                resolution,
            } => AgentEventKind::ExternalApprovalResolved {
                approval_id,
                provider,
                turn_sequence,
                revision,
                resolution,
            },
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation,
                state,
            } => AgentEventKind::DeliveryChanged {
                operation_sequence,
                operation,
                state,
            },
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => AgentEventKind::Usage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            },
            AgentLogKind::RuntimeMetadataChanged { metadata } => {
                AgentEventKind::RuntimeMetadataChanged { metadata }
            }
            AgentLogKind::UsageUpdated {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
                turn_sequence,
                turn_input_tokens,
                turn_output_tokens,
            } => AgentEventKind::UsageUpdated {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
                turn_sequence,
                turn_input_tokens,
                turn_output_tokens,
            },
            AgentLogKind::ModeChanged { mode } => AgentEventKind::ModeChanged { mode },
            AgentLogKind::PlanSet { entries } => AgentEventKind::PlanSet { entries },
            AgentLogKind::ConfigOptionsSet { options } => {
                AgentEventKind::ConfigOptionsSet { options }
            }
            AgentLogKind::AvailableCommandsSet { commands } => {
                AgentEventKind::AvailableCommandsSet { commands }
            }
            AgentLogKind::ConversationGap { frames } => AgentEventKind::HistoryGap { frames },
            AgentLogKind::SignInRequired { .. } => AgentEventKind::AuthenticationRequired,
            AgentLogKind::StatusChanged { from, to } => AgentEventKind::StatusChanged { from, to },
            AgentLogKind::FirstMessageChanged { state, .. } => {
                AgentEventKind::FirstMessageChanged { state }
            }
            AgentLogKind::TitleSet { title, source } => AgentEventKind::TitleSet { title, source },
            AgentLogKind::Stopped { reason } => AgentEventKind::Stopped { reason },
            AgentLogKind::Deleted => AgentEventKind::Deleted,
        };
        Self {
            schema_version: AGENT_IO_SCHEMA_VERSION,
            agent_id: entry.agent_id,
            cursor: entry.seq,
            appended_at: entry.appended_at,
            kind,
        }
    }
}

/// Ephemeral fragments never claim a durable cursor. Only the final assistant
/// message is replayable; reasoning summaries remain in the current live lane.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEphemeralEvent {
    TextDelta {
        agent_id: AgentId,
        turn_sequence: u64,
        message_id: String,
        delta: String,
    },
    ReasoningSummaryDelta {
        agent_id: AgentId,
        turn_sequence: u64,
        message_id: String,
        delta: String,
    },
}

impl From<crate::sandbox::AgentLiveFragment> for AgentEphemeralEvent {
    fn from(fragment: crate::sandbox::AgentLiveFragment) -> Self {
        use crate::sandbox::AgentLiveFragmentKind;
        match fragment.kind {
            AgentLiveFragmentKind::Message => Self::TextDelta {
                agent_id: fragment.agent_id,
                turn_sequence: fragment.turn_sequence,
                message_id: fragment.message_id,
                delta: fragment.delta,
            },
            AgentLiveFragmentKind::Thought => Self::ReasoningSummaryDelta {
                agent_id: fragment.agent_id,
                turn_sequence: fragment.turn_sequence,
                message_id: fragment.message_id,
                delta: fragment.delta,
            },
        }
    }
}
