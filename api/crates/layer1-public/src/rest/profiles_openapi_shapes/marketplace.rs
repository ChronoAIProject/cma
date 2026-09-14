use serde::Serialize;
use utoipa::ToSchema;

use super::{
    AgentProfileRevisionView, AgentProfileView, MarketplaceKindValue, SandboxProfileRevisionView,
    SandboxProfileView, WorkspaceProfileRevisionView, WorkspaceProfileView,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MarketplaceSummaryView {
    pub kind: MarketplaceKindValue,
    pub id: String,
    pub name: String,
    pub description: String,
    pub owner_display: String,
    pub latest_revision: u32,
    pub published_at: String,
    pub uses_count: u64,
}

impl From<layer2_core::profiles::MarketplaceSummary> for MarketplaceSummaryView {
    fn from(value: layer2_core::profiles::MarketplaceSummary) -> Self {
        Self {
            kind: value.kind.into(),
            id: value.id,
            name: value.name,
            description: value.description,
            owner_display: value.owner_display,
            latest_revision: value.latest_revision,
            published_at: value.published_at,
            uses_count: value.uses_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MarketplacePageView {
    pub items: Vec<MarketplaceSummaryView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResolvedProfileNameView {
    pub kind: MarketplaceKindValue,
    pub id: String,
    pub revision: u32,
    pub name: String,
}

impl From<layer2_core::profiles::ResolvedProfileName> for ResolvedProfileNameView {
    fn from(value: layer2_core::profiles::ResolvedProfileName) -> Self {
        Self {
            kind: value.kind.into(),
            id: value.id,
            revision: value.revision,
            name: value.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedProfileRevisionView {
    Agent {
        name: String,
        revision: AgentProfileRevisionView,
    },
    Workspace {
        name: String,
        revision: WorkspaceProfileRevisionView,
    },
    Sandbox {
        name: String,
        revision: SandboxProfileRevisionView,
    },
}

impl From<layer2_core::profiles::ResolvedProfileRevision> for ResolvedProfileRevisionView {
    fn from(value: layer2_core::profiles::ResolvedProfileRevision) -> Self {
        match value.revision {
            layer2_core::profiles::MarketplaceRevision::Agent(revision) => Self::Agent {
                name: value.name,
                revision: revision.into(),
            },
            layer2_core::profiles::MarketplaceRevision::Workspace(revision) => Self::Workspace {
                name: value.name,
                revision: revision.into(),
            },
            layer2_core::profiles::MarketplaceRevision::Sandbox(revision) => Self::Sandbox {
                name: value.name,
                revision: revision.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxProfileTrustView {
    pub requires_confirmation: bool,
    pub commands_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum MarketplaceEntryView {
    Agent {
        summary: MarketplaceSummaryView,
        revision: AgentProfileRevisionView,
        resolved_profiles: Vec<ResolvedProfileNameView>,
        resolved_revisions: Vec<ResolvedProfileRevisionView>,
    },
    Workspace {
        summary: MarketplaceSummaryView,
        revision: WorkspaceProfileRevisionView,
        resolved_profiles: Vec<ResolvedProfileNameView>,
        resolved_revisions: Vec<ResolvedProfileRevisionView>,
    },
    Sandbox {
        summary: MarketplaceSummaryView,
        revision: SandboxProfileRevisionView,
        resolved_profiles: Vec<ResolvedProfileNameView>,
        resolved_revisions: Vec<ResolvedProfileRevisionView>,
        trust: SandboxProfileTrustView,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum ForkedProfileView {
    Agent(AgentProfileView),
    Workspace(WorkspaceProfileView),
    Sandbox(SandboxProfileView),
}
