use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{SubjectId, WorkspaceProfileId};

use super::common::{
    validate_metadata, AgentProfileRef, ProfileError, ProfileVisibility, WorkspaceProfileRef,
};
use super::content_digest;
use super::validation::{validate_command, validate_workspace_name, MAX_COMMAND_BYTES};

const AFTER_CREDENTIALS: &str = "# cma:after-credentials\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkspaceLocation {
    RepositorySnapshot {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        connection_id: Option<String>,
    },
    PublicRepositorySnapshot {
        url: String,
        commit: String,
        operation_id: String,
    },
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
    pub(super) fn validate(&self) -> Result<(), ProfileError> {
        match self {
            Self::Directory { name } => validate_workspace_name(name),
            Self::PublicRepositorySnapshot {
                url,
                commit,
                operation_id,
            } => {
                if operation_id.is_empty()
                    || operation_id.len() > 128
                    || !operation_id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(ProfileError::new(
                        "invalid_repository_operation",
                        "location.operation_id",
                    ));
                }
                if commit.len() != 40
                    || !commit
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                {
                    return Err(ProfileError::new(
                        "invalid_repository_reference",
                        "location.reference",
                    ));
                }
                Self::RepositorySnapshot {
                    url: url.clone(),
                    reference: Some(commit.clone()),
                    connection_id: None,
                }
                .validate()
            }
            Self::GitRepository { url, reference }
            | Self::RepositorySnapshot { url, reference, .. } => {
                let parsed = url::Url::parse(url)
                    .map_err(|_| ProfileError::new("invalid_repository", "location.url"))?;
                if parsed.scheme() != "https"
                    || parsed.host_str() != Some("github.com")
                    || github_authority(url) != Some("github.com")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.port().is_some()
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
                if reference.as_ref().is_some_and(|value| {
                    value.is_empty()
                        || value.len() > 255
                        || value.starts_with('-')
                        || value.contains('\0')
                        || value.chars().any(char::is_whitespace)
                        || value.bytes().any(|byte| byte.is_ascii_control())
                }) {
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

fn github_authority(url: &str) -> Option<&str> {
    url.strip_prefix("https://")?.split(['/', '?', '#']).next()
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
    /// Credential-dependent hooks are explicitly marked by the profile author.
    /// All ordinary setup work can finish before a warm environment is assigned.
    pub fn preparation_commands(&self) -> Vec<String> {
        self.setup_commands
            .iter()
            .filter(|command| !command.starts_with(AFTER_CREDENTIALS))
            .cloned()
            .collect()
    }

    pub fn credential_commands(&self) -> Vec<String> {
        self.setup_commands
            .iter()
            .filter(|command| command.starts_with(AFTER_CREDENTIALS))
            .cloned()
            .collect()
    }

    pub fn validate(&self) -> Result<(), ProfileError> {
        self.location.validate()?;
        if !self.name.is_empty() {
            validate_workspace_name(&self.name)?;
        } else if !matches!(self.location, WorkspaceLocation::GitRepository { .. }) {
            return Err(ProfileError::new("invalid_workspace_name", "name"));
        }
        if self.setup_commands.len() > 16 {
            return Err(ProfileError::new("too_many_commands", "commands"));
        }
        for command in &self.setup_commands {
            if command.len() > MAX_COMMAND_BYTES {
                return Err(ProfileError::new("invalid_command", "commands"));
            }
            validate_command(command.strip_prefix(AFTER_CREDENTIALS).unwrap_or(command))?;
        }
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

    /// Resolve the canonical directory component from the profile snapshot.
    pub fn workspace_name(&self) -> Result<String, ProfileError> {
        let name = if !self.name.is_empty() {
            self.name.clone()
        } else {
            let WorkspaceLocation::GitRepository { url, .. } = &self.location else {
                return Err(ProfileError::new("invalid_workspace_name", "name"));
            };
            let parsed = url::Url::parse(url)
                .map_err(|_| ProfileError::new("invalid_repository", "location.url"))?;
            parsed
                .path_segments()
                .and_then(|mut segments| segments.rfind(|part| !part.is_empty()))
                .map(|name| name.strip_suffix(".git").unwrap_or(name).to_string())
                .ok_or_else(|| ProfileError::new("invalid_repository", "location.url"))?
        };
        validate_workspace_name(&name)?;
        Ok(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfile {
    /// Immutable service-owned snapshot, excluded from profile management.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    pub id: WorkspaceProfileId,
    pub owner: SubjectId,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<WorkspaceProfileRef>,
    #[serde(default)]
    pub uses_count: u64,
    pub draft: WorkspaceProfileContent,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl WorkspaceProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.internal && self.visibility != ProfileVisibility::Private {
            return Err(ProfileError::new(
                "internal_profile_must_be_private",
                "visibility",
            ));
        }
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
