use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::store::{ProfilePage, ProfilePageRequest, ProfileStoreError};
use super::{AgentProfile, ProfileVisibility, SandboxProfile, WorkspaceProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Agent,
    Workspace,
    Sandbox,
}

impl ProfileKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Workspace => "workspace",
            Self::Sandbox => "sandbox",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ProfileLocator {
    Agent(AgentProfileId),
    Workspace(WorkspaceProfileId),
    Sandbox(SandboxProfileId),
}

impl ProfileLocator {
    pub const fn kind(&self) -> ProfileKind {
        match self {
            Self::Agent(_) => ProfileKind::Agent,
            Self::Workspace(_) => ProfileKind::Workspace,
            Self::Sandbox(_) => ProfileKind::Sandbox,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Agent(id) => id.as_str(),
            Self::Workspace(id) => id.as_str(),
            Self::Sandbox(id) => id.as_str(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileAdminQuery {
    pub kind: Option<ProfileKind>,
    pub visibility: Option<ProfileVisibility>,
    pub owner: Option<SubjectId>,
    pub page: ProfilePageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileAdminSummary {
    pub kind: ProfileKind,
    pub id: String,
    pub owner: SubjectId,
    pub name: String,
    pub visibility: ProfileVisibility,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl From<&AgentProfile> for ProfileAdminSummary {
    fn from(profile: &AgentProfile) -> Self {
        Self {
            kind: ProfileKind::Agent,
            id: profile.id.to_string(),
            owner: profile.owner.clone(),
            name: profile.name.clone(),
            visibility: profile.visibility,
            updated_at: profile.updated_at.clone(),
            retired_at: profile.retired_at.clone(),
        }
    }
}

impl From<&WorkspaceProfile> for ProfileAdminSummary {
    fn from(profile: &WorkspaceProfile) -> Self {
        Self {
            kind: ProfileKind::Workspace,
            id: profile.id.to_string(),
            owner: profile.owner.clone(),
            name: profile.name.clone(),
            visibility: profile.visibility,
            updated_at: profile.updated_at.clone(),
            retired_at: profile.retired_at.clone(),
        }
    }
}

impl From<&SandboxProfile> for ProfileAdminSummary {
    fn from(profile: &SandboxProfile) -> Self {
        Self {
            kind: ProfileKind::Sandbox,
            id: profile.id.to_string(),
            owner: profile.owner.clone(),
            name: profile.name.clone(),
            visibility: profile.visibility,
            updated_at: profile.updated_at.clone(),
            retired_at: profile.retired_at.clone(),
        }
    }
}

#[async_trait]
pub trait ProfileAdminStore: Send + Sync {
    async fn list_profiles(
        &self,
        query: &ProfileAdminQuery,
    ) -> Result<ProfilePage<ProfileAdminSummary>, ProfileStoreError>;

    async fn retire_profile(
        &self,
        locator: &ProfileLocator,
        retired_at: String,
    ) -> Result<ProfileAdminSummary, ProfileStoreError>;
}
