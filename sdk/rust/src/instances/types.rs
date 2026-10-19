//! Sandbox, Workspace, and Worktree REST resource models.

use crate::profiles::{AgentProvider, ModelAccess, ProfileReference};
use serde::{Deserialize, Serialize};

string_enum! { /// Persisted Sandbox lifecycle status.
    pub enum SandboxStatus {
        Provisioning => "provisioning", Installing => "installing", Active => "active",
        Pausing => "pausing", Paused => "paused", Reviving => "reviving",
        CredentialRotating => "credential_rotating", Rotating => "rotating", Failed => "failed",
        Retiring => "retiring", Retired => "retired"
    }
}
string_enum! { /// Persisted Workspace status.
    pub enum WorkspaceStatus { Preparing => "preparing", Ready => "ready", Failed => "failed", Removing => "removing", Removed => "removed" }
}
string_enum! { /// Persisted Worktree status.
    pub enum WorktreeStatus { Creating => "creating", Ready => "ready", Failed => "failed", Removing => "removing", Removed => "removed" }
}
string_enum! { /// Persisted Agent status.
    pub enum AgentStatus {
        Starting => "starting", Ready => "ready", Working => "working", WaitingApproval => "waiting_approval",
        Paused => "paused", Stopped => "stopped", Deleted => "deleted", Failed => "failed"
    }
}

/// Safe, machine-classified resource failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// Stable failure class.
    pub class: String,
    /// Human-readable safe detail.
    pub detail: String,
}

/// Admission limits captured when a Sandbox was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SandboxLimits {
    /// Maximum Workspaces.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub max_workspaces: u8,
    /// Maximum Agents per Workspace.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub max_agents_per_workspace: u8,
}

/// A Sandbox resource returned by creation and lifecycle commands.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Sandbox {
    /// Sandbox identifier.
    pub id: String,
    /// Immutable creator identity; absent on older servers.
    pub owner: Option<String>,
    /// Creator's retained public name, when available.
    pub owner_name: Option<String>,
    /// Display name.
    pub name: String,
    /// Current lifecycle status.
    pub status: SandboxStatus,
    /// Immutable source profile.
    pub profile: ProfileReference,
    /// Effective creation and explicitly granted services; absent on older servers.
    pub nyxid_services: Option<Vec<String>>,
    /// Captured admission limits.
    pub limits: SandboxLimits,
    /// Authoritative lifecycle eligibility and recorded progress; absent on older servers.
    pub lifecycle: Option<super::SandboxLifecycle>,
    /// Managed runtime progress, when supplied on owner detail or update responses.
    pub runtime_update: Option<super::SandboxRuntimeUpdate>,
    /// Currently bound runtime version, when present.
    pub runtime_version: Option<String>,
    /// Last recorded failure.
    pub failure: Option<Failure>,
    /// Last owner or Agent activity timestamp.
    pub last_activity_at: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Last mutation timestamp.
    pub updated_at: String,
    /// Retirement timestamp.
    pub retired_at: Option<String>,
    /// Persisted aggregate revision.
    pub revision: u64,
}

/// Sandbox list item with counts of its resource hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxSummary {
    /// Sandbox fields, flattened on the wire.
    #[serde(flatten)]
    pub sandbox: Sandbox,
    /// Workspace count.
    pub workspace_count: u64,
    /// Worktree count.
    pub worktree_count: u64,
    /// Agent count.
    pub agent_count: u64,
}

/// Parent-scoped Workspace summary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceSummary {
    /// Workspace identifier.
    pub id: String,
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Display name.
    pub name: String,
    /// Preparation or removal status.
    pub status: WorkspaceStatus,
    /// Immutable source profile.
    pub profile: ProfileReference,
    /// Allocated Worktree count.
    pub worktree_count: u64,
    /// Agent count.
    pub agent_count: u64,
}

/// Complete owner-facing Sandbox hierarchy detail.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxDetail {
    /// Sandbox and hierarchy counts, flattened on the wire.
    #[serde(flatten)]
    pub summary: SandboxSummary,
    /// Pinned Agent provider; absent on older servers.
    pub agent_provider: Option<AgentProvider>,
    /// Pinned model access; absent on older servers.
    pub model_access: Option<ModelAccess>,
    /// Workspace summaries in this Sandbox.
    pub workspaces: Vec<WorkspaceSummary>,
}

/// Materialized Workspace source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceSource {
    /// Git repository source.
    GitRepository {
        /// Repository URL.
        url: String,
        /// Requested Git revision, branch, or tag.
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    /// Empty directory source.
    Empty,
}

/// Workspace creation or removal result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Workspace {
    /// Workspace identifier.
    pub id: String,
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Display name.
    pub name: String,
    /// Absolute runtime path.
    pub path: String,
    /// Materialized source.
    pub source: WorkspaceSource,
    /// Resolved Git commit, when applicable.
    pub resolved_commit: Option<String>,
    /// Current status.
    pub status: WorkspaceStatus,
    /// Immutable source profile.
    pub profile: ProfileReference,
    /// Last safe failure.
    pub failure: Option<Failure>,
    /// Creation timestamp.
    pub created_at: String,
    /// Preparation completion timestamp.
    pub ready_at: Option<String>,
    /// Removal timestamp.
    pub removed_at: Option<String>,
    /// Persisted revision.
    pub revision: u64,
}

/// Read-only Worktree allocation for an Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Worktree {
    /// Worktree identifier.
    pub id: String,
    /// Owning Workspace identifier.
    pub workspace_id: String,
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Worktree name.
    pub name: String,
    /// Absolute runtime path.
    pub path: String,
    /// Allocated Git branch.
    pub branch: String,
    /// Starting commit.
    pub base_commit: String,
    /// Current allocation status.
    pub status: WorktreeStatus,
    /// Attached Agent, when one exists.
    pub agent_id: Option<String>,
}

/// Compact Agent row included in Workspace hierarchy detail.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HierarchyAgentSummary {
    /// Canonical runtime metadata for this owned Agent.
    pub runtime_metadata: Option<crate::agents::AgentRuntimeMetadata>,
    /// Agent identifier.
    pub id: String,
    /// Allocated Worktree identifier.
    pub worktree_id: String,
    /// Owning Workspace identifier.
    pub workspace_id: String,
    /// Owning Sandbox identifier.
    pub sandbox_id: String,
    /// Current Agent status.
    pub status: AgentStatus,
    /// Immutable Agent profile reference.
    pub profile: ProfileReference,
    /// Agent title, when available.
    pub title: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Workspace detail with allocated Worktrees and Agents.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceDetail {
    /// Workspace fields, flattened on the wire.
    #[serde(flatten)]
    pub workspace: Workspace,
    /// Worktrees within this Workspace.
    pub worktrees: Vec<Worktree>,
    /// Agents within this Workspace.
    pub agents: Vec<HierarchyAgentSummary>,
    /// Worktree count.
    pub worktree_count: u64,
    /// Agent count.
    pub agent_count: u64,
}

/// Request to begin provisioning a Sandbox from an immutable profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSandboxRequest {
    /// Immutable Sandbox profile reference.
    pub sandbox_profile: ProfileReference,
    /// Optional requested display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Explicit acknowledgement of the referenced commands.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trust_confirmed: bool,
    /// Digest shown to the caller before trust confirmation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands_digest: Option<String>,
}

/// Request to prepare a Workspace inside an existing Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWorkspaceRequest {
    /// Immutable Workspace profile reference.
    pub workspace_profile: ProfileReference,
    /// Explicit acknowledgement of referenced commands.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trust_confirmed: bool,
    /// Digest shown to the caller before trust confirmation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands_digest: Option<String>,
}

/// Prepare a Workspace directly from an inline source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInlineWorkspaceRequest {
    /// Empty directory or GitHub repository.
    pub source: WorkspaceSource,
    /// Optional directory name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Explicit owner authorization for additional services on a fully paused Sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantSandboxServiceAccessRequest {
    /// Exact NyxID service identifiers. Existing access is an idempotent no-op.
    pub services: Vec<String>,
}
