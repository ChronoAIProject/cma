//! Public wire shapes for Sandbox, Workspace, and Worktree hierarchy routes.

use layer2_core::sandbox::{
    AgentHierarchySummary, Sandbox, SandboxEvent, SandboxHierarchySummary, SandboxHierarchyView,
    SandboxStatus, TypedFailure, Workspace, WorkspaceHierarchySummary, WorkspaceHierarchyView,
    WorkspaceSource, WorkspaceStatus, WorktreeHierarchySummary, WorktreeStatus,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::profiles_openapi_shapes::{AgentProviderValue, ModelAccessValue};

#[path = "sandbox_lifecycle_view.rs"]
mod lifecycle;
pub use lifecycle::{
    SandboxBackgroundSaveView, SandboxCheckpointView, SandboxInterruptionView,
    SandboxLifecycleEligibilityView, SandboxLifecycleOperationView, SandboxLifecycleView,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileReferenceView {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSandboxRequest {
    pub sandbox_profile: ProfileReferenceView,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub trust_confirmed: bool,
    #[serde(default)]
    pub commands_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantSandboxServiceAccessRequest {
    /// Exact NyxID service identifiers authorized by the Sandbox owner.
    pub services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRequest {
    pub workspace_profile: ProfileReferenceView,
    #[serde(default)]
    pub trust_confirmed: bool,
    #[serde(default)]
    pub commands_digest: Option<String>,
}

/// Direct Agent Manager workspace admission. This request carries the source
/// selected by the caller and does not reference a Workspace Profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInlineWorkspaceRequest {
    pub source: InlineWorkspaceSourceRequest,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InlineWorkspaceSourceRequest {
    GitRepository {
        url: String,
        #[serde(default)]
        reference: Option<String>,
    },
    Empty,
}

impl From<InlineWorkspaceSourceRequest> for layer2_core::sandbox::InlineWorkspaceSource {
    fn from(value: InlineWorkspaceSourceRequest) -> Self {
        match value {
            InlineWorkspaceSourceRequest::GitRepository { url, reference } => {
                Self::GitRepository { url, reference }
            }
            InlineWorkspaceSourceRequest::Empty => Self::Empty,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FailureView {
    pub class: String,
    pub detail: String,
    pub remediation: String,
}

impl From<&TypedFailure> for FailureView {
    fn from(value: &TypedFailure) -> Self {
        Self {
            class: value.class.clone(),
            detail: value.detail.clone(),
            remediation: super::remediation::guidance(&value.class).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxLimitsView {
    pub max_workspaces: u8,
    pub max_agents_per_workspace: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxView {
    pub id: String,
    pub owner: String,
    pub owner_name: Option<String>,
    pub name: String,
    #[schema(value_type = String)]
    pub status: SandboxStatus,
    pub profile: ProfileReferenceView,
    /// Creation scope plus explicitly authorized instance services.
    pub nyxid_services: Vec<String>,
    pub limits: SandboxLimitsView,
    pub lifecycle: SandboxLifecycleView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureView>,
    pub last_activity_at: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    pub revision: u64,
}

impl From<&Sandbox> for SandboxView {
    fn from(value: &Sandbox) -> Self {
        Self {
            id: value.id.to_string(),
            owner: value.owner.0.clone(),
            owner_name: None,
            name: value.name.clone(),
            status: value.status,
            profile: ProfileReferenceView {
                id: value.profile.id.to_string(),
                revision: value.profile.revision,
            },
            nyxid_services: value
                .credential
                .effective_reaches()
                .iter()
                .map(ToString::to_string)
                .collect(),
            limits: SandboxLimitsView {
                max_workspaces: value.limits.max_workspaces,
                max_agents_per_workspace: value.limits.max_agents_per_workspace,
            },
            lifecycle: SandboxLifecycleView::from(value),
            runtime_version: value
                .runtime
                .as_ref()
                .map(|runtime| runtime.runtime_version.clone()),
            failure: value.failure.as_ref().map(Into::into),
            last_activity_at: value.last_activity_at.clone(),
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
            retired_at: value.retired_at.clone(),
            revision: value.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerSandboxSummaryView {
    #[serde(flatten)]
    pub sandbox: SandboxView,
    pub workspace_count: u64,
    pub worktree_count: u64,
    pub agent_count: u64,
}

impl From<SandboxHierarchySummary> for OwnerSandboxSummaryView {
    fn from(value: SandboxHierarchySummary) -> Self {
        Self {
            sandbox: SandboxView::from(&value.sandbox),
            workspace_count: value.counts.workspaces,
            worktree_count: value.counts.worktrees,
            agent_count: value.counts.agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerSandboxPageView {
    pub items: Vec<OwnerSandboxSummaryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerWorkspaceSummaryView {
    pub id: String,
    pub sandbox_id: String,
    pub name: String,
    #[schema(value_type = String)]
    pub status: WorkspaceStatus,
    pub profile: ProfileReferenceView,
    pub worktree_count: u64,
    pub agent_count: u64,
}

impl From<WorkspaceHierarchySummary> for OwnerWorkspaceSummaryView {
    fn from(value: WorkspaceHierarchySummary) -> Self {
        Self {
            id: value.id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            name: value.name,
            status: value.status,
            profile: ProfileReferenceView {
                id: value.profile_id,
                revision: value.profile_revision,
            },
            worktree_count: value.counts.worktrees,
            agent_count: value.counts.agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerSandboxDetailView {
    #[serde(flatten)]
    pub sandbox: SandboxView,
    /// Agent provider pinned when this Sandbox was created.
    pub agent_provider: AgentProviderValue,
    /// Model access pinned when this Sandbox was created.
    pub model_access: ModelAccessValue,
    pub workspaces: Vec<OwnerWorkspaceSummaryView>,
    pub workspace_count: u64,
    pub worktree_count: u64,
    pub agent_count: u64,
}

impl From<SandboxHierarchyView> for OwnerSandboxDetailView {
    fn from(value: SandboxHierarchyView) -> Self {
        Self {
            sandbox: SandboxView::from(&value.sandbox),
            agent_provider: match value.sandbox.profile.content.agent_provider {
                layer2_core::profiles::AgentProvider::Codex => AgentProviderValue::Codex,
            },
            model_access: match value.sandbox.profile.content.model_access {
                layer2_core::profiles::ModelAccess::ChronoaiManaged => {
                    ModelAccessValue::ChronoaiManaged
                }
                layer2_core::profiles::ModelAccess::OwnCredentials => {
                    ModelAccessValue::OwnCredentials
                }
            },
            workspaces: value.workspaces.into_iter().map(Into::into).collect(),
            workspace_count: value.counts.workspaces,
            worktree_count: value.counts.worktrees,
            agent_count: value.counts.agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceSourceView {
    GitRepository {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Empty,
}

impl From<&WorkspaceSource> for WorkspaceSourceView {
    fn from(value: &WorkspaceSource) -> Self {
        match value {
            WorkspaceSource::GitRepository { url, reference } => Self::GitRepository {
                url: url.clone(),
                reference: reference.clone(),
            },
            WorkspaceSource::Empty => Self::Empty,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorkspaceView {
    pub id: String,
    pub sandbox_id: String,
    pub name: String,
    pub path: String,
    pub source: WorkspaceSourceView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    #[schema(value_type = String)]
    pub status: WorkspaceStatus,
    pub profile: ProfileReferenceView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<FailureView>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
    pub revision: u64,
}

impl From<&Workspace> for WorkspaceView {
    fn from(value: &Workspace) -> Self {
        Self {
            id: value.id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            name: value.name.clone(),
            path: value.path.clone(),
            source: WorkspaceSourceView::from(&value.source),
            resolved_commit: value.resolved_commit.clone(),
            status: value.status,
            profile: ProfileReferenceView {
                id: value.profile.id.to_string(),
                revision: value.profile.revision,
            },
            failure: value.failed.as_ref().map(Into::into),
            created_at: value.created_at.clone(),
            ready_at: value.ready_at.clone(),
            removed_at: value.removed_at.clone(),
            revision: value.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorktreeView {
    pub id: String,
    pub workspace_id: String,
    pub sandbox_id: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    #[schema(value_type = String)]
    pub status: WorktreeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl From<WorktreeHierarchySummary> for WorktreeView {
    fn from(value: WorktreeHierarchySummary) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            name: value.name,
            path: value.path,
            branch: value.branch,
            base_commit: value.base_commit,
            status: value.status,
            agent_id: value.agent_id.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct OwnerAgentSummaryView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runtime_metadata: Option<super::agent_observation_shapes::AgentRuntimeMetadataView>,
    pub id: String,
    pub worktree_id: String,
    pub workspace_id: String,
    pub sandbox_id: String,
    #[schema(value_type = String)]
    pub status: layer2_core::sandbox::AgentStatus,
    pub profile: ProfileReferenceView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub updated_at: String,
}

impl From<AgentHierarchySummary> for OwnerAgentSummaryView {
    fn from(value: AgentHierarchySummary) -> Self {
        Self {
            runtime_metadata: value.runtime_metadata.map(Into::into),
            id: value.id.to_string(),
            worktree_id: value.worktree_id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            sandbox_id: value.sandbox_id.to_string(),
            status: value.status,
            profile: ProfileReferenceView {
                id: value.profile_id,
                revision: value.profile_revision,
            },
            title: value.title,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorkspaceDetailView {
    #[serde(flatten)]
    pub workspace: WorkspaceView,
    pub worktrees: Vec<WorktreeView>,
    pub agents: Vec<OwnerAgentSummaryView>,
    pub worktree_count: u64,
    pub agent_count: u64,
}

impl From<WorkspaceHierarchyView> for WorkspaceDetailView {
    fn from(value: WorkspaceHierarchyView) -> Self {
        Self {
            workspace: WorkspaceView::from(&value.workspace),
            worktrees: value.worktrees.into_iter().map(Into::into).collect(),
            agents: value.agents.into_iter().map(Into::into).collect(),
            worktree_count: value.counts.worktrees,
            agent_count: value.counts.agents,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorkspaceListView {
    pub items: Vec<OwnerWorkspaceSummaryView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorktreeListView {
    pub items: Vec<WorktreeView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxEventView {
    pub sandbox_id: String,
    pub seq: u64,
    pub appended_at: String,
    #[schema(value_type = Object)]
    pub kind: layer2_core::sandbox::SandboxEventKind,
}

impl From<SandboxEvent> for SandboxEventView {
    fn from(value: SandboxEvent) -> Self {
        Self {
            sandbox_id: value.sandbox_id.to_string(),
            seq: value.seq,
            appended_at: value.appended_at,
            kind: value.kind,
        }
    }
}
