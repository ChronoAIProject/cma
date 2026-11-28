use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, WorkspaceId, WorktreeId};

use super::{
    AgentOperationDelivery, AgentOperationKind, AgentStatus, AgentWorkStep, SandboxStatus,
    SandboxWorkStep, TypedFailure,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedToolOutput {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPlanStep {
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigOption {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAvailableCommand {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalApprovalResolution {
    Approved,
    Denied,
    Expired,
    Disconnected,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentLogKind {
    Created {
        workspace_id: WorkspaceId,
        worktree_id: WorktreeId,
    },
    ConversationOpened {
        conversation_id: String,
        runtime_generation: u64,
    },
    RuntimeGenerationRetired {
        previous_generation: u64,
        runtime_generation: u64,
    },
    SkillInstallationChanged {
        skill_id: String,
        status: super::AgentSkillInstallStatus,
    },
    WorkStepCompleted {
        step: AgentWorkStep,
    },
    WorkStepFailed {
        step: AgentWorkStep,
        failure: TypedFailure,
    },
    TurnStarted {
        turn_sequence: u64,
        /// Exact owner-selected runtime model fenced with this turn. Absent on
        /// legacy records; an empty value explicitly delegates to runtime default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_key: Option<String>,
    },
    TurnMetadataSet {
        turn_sequence: u64,
        metadata: std::collections::BTreeMap<String, String>,
    },
    OwnerMessage {
        content: String,
    },
    AgentMessage {
        content: String,
    },
    /// A committed suffix of the current assistant message. The final message
    /// remains the transcript row.
    AgentMessageDelta {
        offset: u32,
        content: String,
    },
    TurnCompleted {
        turn_sequence: u64,
    },
    TurnFailed {
        turn_sequence: u64,
        failure: TypedFailure,
    },
    TurnInterrupted {
        turn_sequence: u64,
    },
    ToolCallStarted {
        call_id: String,
        title: String,
        kind: String,
        status: String,
    },
    ToolCallUpdated {
        call_id: String,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<RecordedToolOutput>,
    },
    ToolOutputReported {
        call_id: String,
        output: RecordedToolOutput,
    },
    ApprovalRequested {
        approval_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_locator: Option<String>,
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
    },
    ExternalApprovalResolved {
        approval_id: String,
        provider: String,
        turn_sequence: u64,
        revision: u64,
        resolution: ExternalApprovalResolution,
    },
    OperationDeliveryChanged {
        operation_sequence: u64,
        operation: AgentOperationKind,
        state: AgentOperationDelivery,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        context_used: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
    },
    RuntimeMetadataChanged {
        metadata: super::AgentRuntimeMetadata,
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
    ConversationGap {
        frames: u64,
    },
    SignInRequired {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    StatusChanged {
        from: AgentStatus,
        to: AgentStatus,
    },
    FirstMessageChanged {
        state: super::FirstMessageDelivery,
        recovery_generation: u32,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentLogEntry {
    pub agent_id: AgentId,
    pub seq: u64,
    pub appended_at: String,
    pub kind: AgentLogKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxEventKind {
    Created,
    WorkStepCompleted {
        step: SandboxWorkStep,
    },
    WorkStepFailed {
        step: SandboxWorkStep,
        failure: TypedFailure,
    },
    StatusChanged {
        from: SandboxStatus,
        to: SandboxStatus,
    },
    WorkspaceCreated {
        workspace_id: WorkspaceId,
    },
    WorkspaceReady {
        workspace_id: WorkspaceId,
        resolved_commit: String,
    },
    WorkspaceFailed {
        workspace_id: WorkspaceId,
        failure: TypedFailure,
    },
    WorkspaceRemoved {
        workspace_id: WorkspaceId,
    },
    WorktreeCreated {
        workspace_id: WorkspaceId,
        worktree_id: WorktreeId,
    },
    WorktreeRemoved {
        workspace_id: WorkspaceId,
        worktree_id: WorktreeId,
    },
    AgentCreated {
        workspace_id: WorkspaceId,
        worktree_id: WorktreeId,
        agent_id: AgentId,
    },
    AgentStatusChanged {
        agent_id: AgentId,
        from: AgentStatus,
        to: AgentStatus,
    },
    AgentDeleted {
        agent_id: AgentId,
    },
    CredentialChanged {
        key_generation: u64,
    },
    ServiceAccessGranted {
        grant: super::SandboxServiceGrant,
    },
    ServiceAccessRevised {
        scope: Box<super::SandboxServiceScopeRevision>,
    },
    CheckpointSealed {
        object_reference: String,
    },
    RuntimeRecoveryAdmitted {
        previous_status: SandboxStatus,
        target_runtime_version: String,
        proof: Box<super::SandboxUnpublishedRecoveryProof>,
    },
    RuntimeRecoveryQuiesced {
        receipt: Box<super::SandboxUnpublishedRecoveryQuiescence>,
    },
    Revived,
    HomeRestored {
        restore_id: String,
        backup_id: String,
        restored_to: String,
    },
    Rotated {
        runtime_version: String,
    },
    Failed {
        failure: TypedFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxEvent {
    pub sandbox_id: SandboxId,
    pub seq: u64,
    pub appended_at: String,
    pub kind: SandboxEventKind,
}
