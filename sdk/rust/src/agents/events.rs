//! Typed durable Agent events returned by REST polling.

use super::FirstMessageDelivery;
use crate::instances::{AgentStatus, Failure};
use serde::Deserialize;
use std::collections::BTreeMap;

string_enum! { /// Durable Agent worker step.
    pub enum AgentWorkStep {
        WaitForWorkspace => "wait_for_workspace",
        AddWorktree => "add_worktree", MaterializeHome => "materialize_home",
        OpenConversation => "open_conversation", PublishReady => "publish_ready",
        PublishConnection => "publish_connection", DispatchFirstMessage => "dispatch_first_message",
        InstallSkill => "install_skill",
        FenceInteraction => "fence_interaction", CloseProcess => "close_process",
        PublishStopped => "publish_stopped", EnsureProcessClosed => "ensure_process_closed",
        RemoveHome => "remove_home", RemoveWorktree => "remove_worktree", PublishDeleted => "publish_deleted", None => "none"
    }
}
string_enum! { /// Semantic kind of an Agent command.
    pub enum AgentOperationKind { SendMessage => "send_message", Steer => "steer", Interrupt => "interrupt", AnswerApproval => "answer_approval" }
}
string_enum! { /// Durable command-delivery state; write confirmation is not execution.
    pub enum AgentOperationDelivery {
        DispatchFenced => "dispatch_fenced", WriteConfirmed => "write_confirmed", ExecutionObserved => "execution_observed",
        PreWriteFailed => "pre_write_failed", RecoveryRequired => "recovery_required", RuntimeLost => "runtime_lost"
    }
}

string_enum! { /// Runtime-confirmed state of an additive Agent skill.
    pub enum AgentSkillInstallStatus { Pending => "pending", Installed => "installed", Failed => "failed" }
}

/// One plan entry emitted by an Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PlanEntry {
    /// Entry title.
    pub title: String,
    /// Adapter-projected status.
    pub status: String,
}

/// One reported Agent configuration option.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigOption {
    /// Option identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Option kind.
    pub kind: String,
}

/// One command advertised by an Agent runtime.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AvailableCommand {
    /// Command name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

event_enum! { "type";
    /// Typed durable Agent event payload; it does not model live-only frames.
    pub enum AgentEventKind {
        AccessRedacted => "access_redacted" {},
        Created => "created" { workspace_id: String, worktree_id: String },
        ConversationOpened => "conversation_opened" { conversation_id: String, runtime_generation: u64 },
        RuntimeGenerationRetired => "runtime_generation_retired" { previous_generation: u64, runtime_generation: u64 },
        WorkStepCompleted => "work_step_completed" { step: AgentWorkStep },
        WorkStepFailed => "work_step_failed" { step: AgentWorkStep, failure: Failure },
        SkillInstallationChanged => "skill_installation_changed" { skill_id: String, status: AgentSkillInstallStatus },
        TurnStarted => "turn_started" { turn_sequence: u64 },
        TurnMetadataSet => "turn_metadata_set" { turn_sequence: u64, metadata: BTreeMap<String, String> },
        OwnerMessage => "owner_message" { content: String },
        AgentMessage => "agent_message" { content: String },
        TurnCompleted => "turn_completed" { turn_sequence: u64 },
        TurnFailed => "turn_failed" { turn_sequence: u64, failure: Failure },
        TurnInterrupted => "turn_interrupted" { turn_sequence: u64 },
        ToolCallStarted => "tool_call_started" { call_id: String, title: String, kind: String, status: String },
        ToolCallUpdated => "tool_call_updated" { call_id: String, status: String, title: Option<String> },
        ApprovalRequested => "approval_requested" { approval_id: String, action: Option<String>, detail: Option<String> },
        ApprovalResolved => "approval_resolved" { approval_id: String, decision: String },
        OperationDeliveryChanged => "operation_delivery_changed" { operation_sequence: u64, operation: AgentOperationKind, state: AgentOperationDelivery },
        Usage => "usage" { input_tokens: u64, output_tokens: u64, context_used: u64, context_window: Option<u64> },
        RuntimeMetadataChanged => "runtime_metadata_changed" { metadata: super::AgentRuntimeMetadata },
        UsageUpdated => "usage_updated" { input_tokens: u64, output_tokens: u64, context_used: u64, context_window: Option<u64>, turn_sequence: Option<u64>, turn_input_tokens: u64, turn_output_tokens: u64 },
        ModeChanged => "mode_changed" { mode: String },
        PlanSet => "plan_set" { entries: Vec<PlanEntry> },
        ConfigOptionsSet => "config_options_set" { options: Vec<ConfigOption> },
        AvailableCommandsSet => "available_commands_set" { commands: Vec<AvailableCommand> },
        ConversationGap => "conversation_gap" { frames: u64 },
        SignInRequired => "sign_in_required" { url: String, code: Option<String>, message: Option<String> },
        StatusChanged => "status_changed" { from: AgentStatus, to: AgentStatus },
        FirstMessageChanged => "first_message_changed" { state: FirstMessageDelivery, recovery_generation: u64 },
        TitleSet => "title_set" { title: String, source: String },
        Stopped => "stopped" { reason: String },
        Deleted => "deleted" {}
    }
}

/// One durable Agent log entry returned by the REST events endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentEvent {
    /// Durable monotonic event sequence.
    pub seq: u64,
    /// Event append timestamp.
    pub appended_at: String,
    /// Typed event payload.
    pub kind: AgentEventKind,
}
