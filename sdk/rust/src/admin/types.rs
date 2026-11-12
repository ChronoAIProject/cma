//! Public operator inventory and safe activity projections.

use crate::{
    agents::InstanceProfileReference,
    instances::{AgentStatus, Failure, SandboxStatus, WorkspaceStatus},
    profiles::{ProfileKind, ProfileVisibility},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Operator Sandbox list item.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxSummary {
    /// Sandbox identity.
    pub id: String,
    /// Owning subject.
    pub owner: String,
    /// Creator's retained public name; absent when unavailable or on older servers.
    pub owner_name: Option<String>,
    /// Sandbox display name.
    pub name: String,
    /// Runtime lifecycle.
    pub status: SandboxStatus,
    /// Observed runtime contract version.
    pub runtime_version: Option<String>,
    /// Number of Agents.
    pub agent_count: u64,
    /// Last activity timestamp.
    pub last_activity_at: String,
    /// Retirement timestamp.
    pub retired_at: Option<String>,
}

/// Non-secret runtime contract metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Runtime {
    /// Pinned runtime version.
    pub runtime_version: String,
}

/// Per-Sandbox hierarchy limits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxLimits {
    /// Workspace admission limit.
    pub max_workspaces: u32,
    /// Agent admission limit per Workspace.
    pub max_agents_per_workspace: u32,
}

/// Operator Sandbox projection deliberately excludes checkpoint and credential state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Sandbox {
    /// Sandbox identity.
    pub id: String,
    /// Owning subject.
    pub owner: String,
    /// Owner name.
    pub name: String,
    /// Runtime lifecycle.
    pub status: SandboxStatus,
    /// Immutable profile identity.
    pub profile: InstanceProfileReference,
    /// Explicit automatic-pause exception, when supplied by the server.
    /// True alone does not establish approval provenance for retained records.
    #[serde(default)]
    pub always_on: Option<bool>,
    /// Observed runtime metadata.
    pub runtime: Option<Runtime>,
    /// Hierarchy limits.
    pub limits: SandboxLimits,
    /// Safe failure evidence.
    pub failure: Option<Failure>,
    /// Last activity timestamp.
    pub last_activity_at: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Last update timestamp.
    pub updated_at: String,
    /// Retirement timestamp.
    pub retired_at: Option<String>,
    /// Durable record version.
    pub revision: u64,
}

/// Child Workspace operator summary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WorkspaceSummary {
    /// Workspace identity.
    pub id: String,
    /// Owner name.
    pub name: String,
    /// Preparation lifecycle.
    pub status: WorkspaceStatus,
    /// Pinned source profile identity.
    pub profile_id: String,
    /// Pinned profile revision.
    pub profile_revision: u32,
}

/// Child Agent operator summary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentSummary {
    /// Agent identity.
    pub id: String,
    /// Parent Workspace identity.
    pub workspace_id: String,
    /// Parent Worktree identity.
    pub worktree_id: String,
    /// Agent lifecycle.
    pub status: AgentStatus,
    /// Pinned source profile identity.
    pub profile_id: String,
    /// Pinned profile revision.
    pub profile_revision: u32,
    /// Optional owner title.
    pub title: Option<String>,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Operator Sandbox detail with bounded child summaries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxDetail {
    /// Creator's retained public name.
    pub owner_name: Option<String>,
    /// Managed runtime target and progress, absent on older deployments.
    pub runtime_update: Option<crate::instances::SandboxRuntimeUpdate>,
    /// Safe Sandbox projection.
    pub sandbox: Sandbox,
    /// Child Workspace summaries.
    pub workspaces: Vec<WorkspaceSummary>,
    /// Child Agent summaries.
    pub agents: Vec<AgentSummary>,
}

/// Operator profile metadata, without exposing private profile content.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProfileSummary {
    /// Profile resource kind.
    pub kind: ProfileKind,
    /// Profile identity.
    pub id: String,
    /// Owning subject.
    pub owner: String,
    /// Creator's retained public name.
    pub owner_name: Option<String>,
    /// Owner profile name.
    pub name: String,
    /// Public discoverability.
    pub visibility: ProfileVisibility,
    /// Last update timestamp.
    pub updated_at: String,
    /// Retirement timestamp.
    pub retired_at: Option<String>,
}

string_enum! { /// Recorded activity action.
    pub enum ActivityAction { Authenticate => "authenticate", Create => "create", Read => "read", List => "list", Update => "update", Delete => "delete", Configure => "configure" }
}
string_enum! { /// Activity target kind.
    pub enum TargetKind { Subject => "subject", Service => "service", Skill => "skill", Resource => "resource" }
}
string_enum! { /// Runtime inventory observation status.
    pub enum RuntimeStatus { Starting => "starting", Ready => "ready", Unavailable => "unavailable" }
}
event_enum! { "status"; /// Safe operation outcome.
    pub enum ActivityOutcome { Succeeded => "succeeded" {}, Refused => "refused" { code: String }, Failed => "failed" { code: String } }
}

/// Public identity of an audited operation target.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActivityTarget {
    /// Target resource kind.
    pub kind: TargetKind,
    /// Target identity.
    pub id: String,
}

/// Safe administrative activity event.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActivityRecord {
    /// Activity identity.
    pub id: String,
    /// Event timestamp.
    pub at: String,
    /// Acting subject, if authenticated.
    pub actor: Option<String>,
    /// Acting user's retained public name.
    pub actor_name: Option<String>,
    /// Public name of a subject target, when available.
    pub target_name: Option<String>,
    /// Operation action.
    pub action: ActivityAction,
    /// Operation target.
    pub target: Option<ActivityTarget>,
    /// Typed result.
    pub outcome: ActivityOutcome,
    /// Safe string-valued operation metadata.
    pub detail: BTreeMap<String, String>,
}

/// One runtime observed by the operator inventory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuntimeEntry {
    /// Runtime identity.
    pub runtime_id: String,
    /// Runtime incarnation generation.
    pub generation: u64,
    /// Owning resource reference.
    pub owner_reference: Option<String>,
    /// Observed availability.
    pub status: RuntimeStatus,
}

/// Current observed runtime inventory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RuntimeInventory {
    /// Observation timestamp.
    pub observed_at: String,
    /// Observed runtimes.
    pub runtimes: Vec<RuntimeEntry>,
}

/// Administrator approval or revocation of an automatic-pause exception.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlwaysOnRequest {
    /// Whether this Sandbox should remain running while idle.
    pub always_on: bool,
    /// Exact Sandbox revision being changed.
    pub expected_revision: u64,
}

/// Number of active Sandboxes that currently satisfy the idle proof.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AutoPausePreview {
    /// Sandboxes that would be paused by the current policy.
    pub candidates: u64,
}

/// Confirmed operator idle-pause result counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseIdleNowRequest {
    /// Confirmation gate required by the destructive operator action.
    pub confirmed: bool,
}

/// Counts returned by an operator idle-pause request.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PauseIdleNow {
    /// Sandboxes newly scheduled for pause.
    pub scheduled: u64,
    /// Sandboxes which already had a matching pause scheduled.
    pub already_satisfied: u64,
    /// Sandboxes rejected by the current activity or policy proof.
    pub skipped: u64,
}
