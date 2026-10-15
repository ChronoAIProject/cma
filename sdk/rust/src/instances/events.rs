//! Typed durable Sandbox log entries returned by REST polling.

use super::{AgentStatus, Failure, SandboxStatus};
use serde::Deserialize;

event_enum! { "kind";
    /// Durable Sandbox orchestration work step, tagged by `kind` on the wire.
    pub enum SandboxWorkStep {
        ProvisionRuntime => "provision_runtime" {},
        RunInstall => "run_install" {},
        MintInitialCredential => "mint_initial_credential" {},
        DeliverInitialCredential => "deliver_initial_credential" {},
        Activate => "activate" {},
        WaitForCredentialRotation => "wait_for_credential_rotation" {},
        MintRotatedCredential => "mint_rotated_credential" {},
        DeliverRotatedCredential => "deliver_rotated_credential" {},
        RevokeRotatedCredential => "revoke_rotated_credential" {},
        FinishCredentialRotation => "finish_credential_rotation" {},
        RunLifecycleOperation => "run_lifecycle_operation" {},
        CreateProfileWorkspace => "create_profile_workspace" { index: u64 },
        StopAgents => "stop_agents" {},
        RevokeRetirementCredentials => "revoke_retirement_credentials" {},
        TeardownRuntime => "teardown_runtime" {},
        MarkRetired => "mark_retired" {},
        RetryRetiredRevocations => "retry_retired_revocations" {},
        WaitForPurge => "wait_for_purge" {},
        Purge => "purge" {},
        None => "none" {}
    }
}

event_enum! { "type";
    /// Typed durable Sandbox event payload.
    pub enum SandboxEventKind {
        Created => "created" {},
        WorkStepCompleted => "work_step_completed" { step: SandboxWorkStep },
        WorkStepFailed => "work_step_failed" { step: SandboxWorkStep, failure: Failure },
        StatusChanged => "status_changed" { from: SandboxStatus, to: SandboxStatus },
        WorkspaceCreated => "workspace_created" { workspace_id: String },
        WorkspaceReady => "workspace_ready" { workspace_id: String, resolved_commit: String },
        WorkspaceFailed => "workspace_failed" { workspace_id: String, failure: Failure },
        WorkspaceRemoved => "workspace_removed" { workspace_id: String },
        WorktreeCreated => "worktree_created" { workspace_id: String, worktree_id: String },
        WorktreeRemoved => "worktree_removed" { workspace_id: String, worktree_id: String },
        AgentCreated => "agent_created" { workspace_id: String, worktree_id: String, agent_id: String },
        AgentStatusChanged => "agent_status_changed" { agent_id: String, from: AgentStatus, to: AgentStatus },
        AgentDeleted => "agent_deleted" { agent_id: String },
        CredentialChanged => "credential_changed" { key_generation: u64 },
        ServiceAccessGranted => "service_access_granted" { grant: SandboxServiceGrant },
        ServiceAccessRevised => "service_access_revised" { scope: SandboxServiceScopeRevision },
        CheckpointSealed => "checkpoint_sealed" { object_reference: String },
        Revived => "revived" {},
        Rotated => "rotated" { runtime_version: String },
        Failed => "failed" { failure: Failure }
    }
}

/// One durable Sandbox event in monotonic sequence order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxEvent {
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Durable monotonic event sequence.
    pub seq: u64,
    /// Event append timestamp.
    pub appended_at: String,
    /// Typed durable payload.
    pub kind: SandboxEventKind,
}

/// Durable, non-secret authorization for additional Sandbox services.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxServiceGrant {
    /// Newly added service identifiers.
    pub services: Vec<String>,
    /// Sandbox owner who authorized the addition.
    pub authorized_by: String,
    /// Authorization timestamp.
    pub authorized_at: String,
    /// Credentials at or below this reservation predate the added services.
    pub credential_generation_floor: u64,
}

/// Latest exact owner-authorized scope; creation scope remains immutable.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxServiceScopeRevision {
    /// Monotonic scope revision within the Sandbox.
    pub revision: u64,
    /// Complete desired service set, including required baseline services.
    pub services: Vec<String>,
    /// Sandbox owner who authorized the replacement.
    pub authorized_by: String,
    /// Authorization timestamp.
    pub authorized_at: String,
    /// A delivered credential must exceed this reservation to cover this scope.
    pub credential_generation_floor: u64,
}
