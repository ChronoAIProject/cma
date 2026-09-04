use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, SandboxId, WorkspaceId, WorktreeId};

use super::{AgentStatus, SandboxStatus, SandboxWorkStep, TypedFailure};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentLogKind {
    OwnerMessage {
        content: String,
    },
    AgentMessage {
        content: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        status: String,
    },
    Approval {
        approval_id: String,
        decision: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    StatusChanged {
        from: AgentStatus,
        to: AgentStatus,
    },
    Stopped {
        reason: String,
    },
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
    CheckpointSealed {
        object_reference: String,
    },
    Revived,
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
