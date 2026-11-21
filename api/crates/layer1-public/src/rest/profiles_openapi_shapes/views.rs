use serde::Serialize;
use utoipa::ToSchema;

use super::{
    AgentProfileContentBody, AgentProfileReference, ProfileVisibilityValue,
    SandboxProfileContentBody, SandboxProfileReference, WorkspaceProfileContentBody,
    WorkspaceProfileReference,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProfileRevisionSummary {
    pub revision: u32,
    pub digest: String,
    pub published_at: String,
}

impl From<layer2_core::profiles::store::PublishedProfileSummary> for ProfileRevisionSummary {
    fn from(value: layer2_core::profiles::store::PublishedProfileSummary) -> Self {
        Self {
            revision: value.revision,
            digest: value.digest,
            published_at: value.published_at,
        }
    }
}

macro_rules! revision_view {
    ($name:ident, $domain:ty, $content:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
        pub struct $name {
            pub profile_id: String,
            pub revision: u32,
            pub content: $content,
            pub digest: String,
            pub published_at: String,
        }

        impl From<$domain> for $name {
            fn from(value: $domain) -> Self {
                Self {
                    profile_id: value.profile_id.to_string(),
                    revision: value.revision,
                    content: value.content.into(),
                    digest: value.digest,
                    published_at: value.published_at,
                }
            }
        }
    };
}

revision_view!(
    AgentProfileRevisionView,
    layer2_core::profiles::AgentProfileRevision,
    AgentProfileContentBody
);
revision_view!(
    WorkspaceProfileRevisionView,
    layer2_core::profiles::WorkspaceProfileRevision,
    WorkspaceProfileContentBody
);
revision_view!(
    SandboxProfileRevisionView,
    layer2_core::profiles::SandboxProfileRevision,
    SandboxProfileContentBody
);

impl From<&AgentProfileRevisionView> for ProfileRevisionSummary {
    fn from(value: &AgentProfileRevisionView) -> Self {
        Self {
            revision: value.revision,
            digest: value.digest.clone(),
            published_at: value.published_at.clone(),
        }
    }
}

impl From<&WorkspaceProfileRevisionView> for ProfileRevisionSummary {
    fn from(value: &WorkspaceProfileRevisionView) -> Self {
        Self {
            revision: value.revision,
            digest: value.digest.clone(),
            published_at: value.published_at.clone(),
        }
    }
}

impl From<&SandboxProfileRevisionView> for ProfileRevisionSummary {
    fn from(value: &SandboxProfileRevisionView) -> Self {
        Self {
            revision: value.revision,
            digest: value.digest.clone(),
            published_at: value.published_at.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct AgentProfileView {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibilityValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<AgentProfileReference>,
    pub uses_count: u64,
    pub draft: AgentProfileContentBody,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_revision: Option<ProfileRevisionSummary>,
}

impl AgentProfileView {
    pub fn new(
        profile: layer2_core::profiles::AgentProfile,
        latest_revision: Option<ProfileRevisionSummary>,
    ) -> Self {
        Self {
            id: profile.id.to_string(),
            owner: profile.owner.to_string(),
            name: profile.name,
            description: profile.description,
            visibility: profile.visibility.into(),
            forked_from: profile.forked_from.map(|reference| AgentProfileReference {
                id: reference.id.to_string(),
                revision: reference.revision,
            }),
            uses_count: profile.uses_count,
            draft: profile.draft.into(),
            draft_version: profile.draft_version,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            retired_at: profile.retired_at,
            latest_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct WorkspaceProfileView {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibilityValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<WorkspaceProfileReference>,
    pub uses_count: u64,
    pub draft: WorkspaceProfileContentBody,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_revision: Option<ProfileRevisionSummary>,
}

impl WorkspaceProfileView {
    pub fn new(
        profile: layer2_core::profiles::WorkspaceProfile,
        latest_revision: Option<ProfileRevisionSummary>,
    ) -> Self {
        Self {
            id: profile.id.to_string(),
            owner: profile.owner.to_string(),
            name: profile.name,
            description: profile.description,
            visibility: profile.visibility.into(),
            forked_from: profile
                .forked_from
                .map(|reference| WorkspaceProfileReference {
                    id: reference.id.to_string(),
                    revision: reference.revision,
                }),
            uses_count: profile.uses_count,
            draft: profile.draft.into(),
            draft_version: profile.draft_version,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            retired_at: profile.retired_at,
            latest_revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SandboxProfileView {
    pub availability: super::SandboxProfileAvailabilityBody,
    pub id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibilityValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<SandboxProfileReference>,
    pub uses_count: u64,
    pub draft: SandboxProfileContentBody,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_revision: Option<ProfileRevisionSummary>,
}

impl SandboxProfileView {
    pub fn new(
        profile: layer2_core::profiles::SandboxProfile,
        latest_revision: Option<ProfileRevisionSummary>,
    ) -> Self {
        Self {
            availability: profile.availability.into(),
            id: profile.id.to_string(),
            owner: profile.owner.to_string(),
            name: profile.name,
            description: profile.description,
            visibility: profile.visibility.into(),
            forked_from: profile
                .forked_from
                .map(|reference| SandboxProfileReference {
                    id: reference.id.to_string(),
                    revision: reference.revision,
                }),
            uses_count: profile.uses_count,
            draft: profile.draft.into(),
            draft_version: profile.draft_version,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            retired_at: profile.retired_at,
            latest_revision,
        }
    }
}

macro_rules! page_view {
    ($name:ident, $item:ty) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
        pub struct $name {
            pub items: Vec<$item>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub next: Option<String>,
        }
    };
}

page_view!(AgentProfilePageView, AgentProfileView);
page_view!(WorkspaceProfilePageView, WorkspaceProfileView);
page_view!(SandboxProfilePageView, SandboxProfileView);
page_view!(AgentProfileRevisionPageView, AgentProfileRevisionView);
page_view!(
    WorkspaceProfileRevisionPageView,
    WorkspaceProfileRevisionView
);
page_view!(SandboxProfileRevisionPageView, SandboxProfileRevisionView);
