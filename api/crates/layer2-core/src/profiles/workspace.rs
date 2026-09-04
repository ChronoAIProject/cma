use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{SubjectId, WorkspaceProfileId};

use super::common::{validate_metadata, AgentProfileRef, ProfileError, ProfileVisibility};
use super::content_digest;
use super::validation::{validate_commands, validate_workspace_name};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceLocation {
    GitRepository {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
    },
    Directory {
        name: String,
    },
}

impl WorkspaceLocation {
    fn validate(&self) -> Result<(), ProfileError> {
        match self {
            Self::Directory { name } => validate_workspace_name(name),
            Self::GitRepository { url, reference } => {
                let parsed = url::Url::parse(url)
                    .map_err(|_| ProfileError::new("invalid_repository", "location.url"))?;
                if parsed.scheme() != "https"
                    || parsed.host_str() != Some("github.com")
                    || parsed.query().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(ProfileError::new("invalid_repository", "location.url"));
                }
                let components: Vec<_> = parsed
                    .path_segments()
                    .into_iter()
                    .flatten()
                    .filter(|part| !part.is_empty())
                    .collect();
                if components.len() != 2 {
                    return Err(ProfileError::new("invalid_repository", "location.url"));
                }
                if reference
                    .as_ref()
                    .is_some_and(|value| value.trim().is_empty())
                {
                    return Err(ProfileError::new(
                        "invalid_repository_reference",
                        "location.reference",
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileContent {
    pub location: WorkspaceLocation,
    pub name: String,
    #[serde(default)]
    pub setup_commands: Vec<String>,
    #[serde(default)]
    pub agent_profiles: Vec<AgentProfileRef>,
}

impl WorkspaceProfileContent {
    pub fn validate(&self) -> Result<(), ProfileError> {
        self.location.validate()?;
        validate_workspace_name(&self.name)?;
        validate_commands(&self.setup_commands, 16)?;
        if self.agent_profiles.len() > 8 {
            return Err(ProfileError::new(
                "too_many_agent_profiles",
                "agent_profiles",
            ));
        }
        let unique: BTreeSet<_> = self
            .agent_profiles
            .iter()
            .map(|reference| (&reference.id, reference.revision))
            .collect();
        if unique.len() != self.agent_profiles.len()
            || self
                .agent_profiles
                .iter()
                .any(|reference| reference.revision == 0)
        {
            return Err(ProfileError::new(
                "invalid_agent_profile_reference",
                "agent_profiles",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfile {
    pub id: WorkspaceProfileId,
    pub owner: SubjectId,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibility,
    pub draft: WorkspaceProfileContent,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl WorkspaceProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        validate_metadata(&self.name, &self.description)?;
        self.draft.validate()
    }

    pub fn publish(
        &self,
        revision: u32,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileError> {
        self.validate()?;
        Ok(WorkspaceProfileRevision {
            profile_id: self.id.clone(),
            revision,
            digest: content_digest(&self.draft)?,
            content: self.draft.clone(),
            published_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileRevision {
    pub profile_id: WorkspaceProfileId,
    pub revision: u32,
    pub content: WorkspaceProfileContent,
    pub digest: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSnapshot {
    pub id: WorkspaceProfileId,
    pub revision: u32,
    pub digest: String,
    pub content: WorkspaceProfileContent,
}

impl From<&WorkspaceProfileRevision> for WorkspaceProfileSnapshot {
    fn from(revision: &WorkspaceProfileRevision) -> Self {
        Self {
            id: revision.profile_id.clone(),
            revision: revision.revision,
            digest: revision.digest.clone(),
            content: revision.content.clone(),
        }
    }
}
