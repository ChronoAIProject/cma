use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::ids::{SandboxProfileId, ServiceId, SubjectId};

use super::common::{validate_metadata, ProfileError, ProfileVisibility, WorkspaceProfileRef};
use super::content_digest;
use super::validation::validate_commands;

const RESERVED_ENVIRONMENT_NAMES: &[&str] = &[
    "PATH",
    "HOME",
    "NYXID_URL",
    "NYXID_ACCESS_TOKEN",
    "CODEX_HOME",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAccess {
    ChronoaiManaged,
    OwnCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileCapacityBounds {
    pub min_cpu_millis: u32,
    pub max_cpu_millis: u32,
    pub min_memory_mib: u32,
    pub max_memory_mib: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentVariable {
    pub name: String,
    pub value: String,
}

impl EnvironmentVariable {
    fn validate(&self) -> Result<(), ProfileError> {
        let mut chars = self.name.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            || !chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            || RESERVED_ENVIRONMENT_NAMES.contains(&self.name.as_str())
            || self.name.starts_with("CHRONO_")
        {
            return Err(ProfileError::new(
                "reserved_environment_variable",
                "environment_variables.name",
            ));
        }
        if self.value.contains('\0') {
            return Err(ProfileError::new(
                "invalid_environment_value",
                "environment_variables.value",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileContent {
    pub cpu_millis: u32,
    pub memory_mib: u32,
    #[serde(default)]
    pub nyxid_services: Vec<ServiceId>,
    pub agent_provider: AgentProvider,
    pub model_access: ModelAccess,
    pub max_workspaces: u8,
    pub max_agents_per_workspace: u8,
    #[serde(default)]
    pub workspace_profiles: Vec<WorkspaceProfileRef>,
    #[serde(default)]
    pub software_install_commands: Vec<String>,
    #[serde(default)]
    pub environment_variables: Vec<EnvironmentVariable>,
}

impl SandboxProfileContent {
    pub fn validate(&self, bounds: ProfileCapacityBounds) -> Result<(), ProfileError> {
        if !(bounds.min_cpu_millis..=bounds.max_cpu_millis).contains(&self.cpu_millis) {
            return Err(ProfileError::new("cpu_out_of_bounds", "cpu_millis"));
        }
        if !(bounds.min_memory_mib..=bounds.max_memory_mib).contains(&self.memory_mib) {
            return Err(ProfileError::new("memory_out_of_bounds", "memory_mib"));
        }
        if !(1..=16).contains(&self.max_workspaces) {
            return Err(ProfileError::new(
                "invalid_workspace_limit",
                "max_workspaces",
            ));
        }
        if !(1..=16).contains(&self.max_agents_per_workspace) {
            return Err(ProfileError::new(
                "invalid_agent_limit",
                "max_agents_per_workspace",
            ));
        }
        if self.workspace_profiles.len() > usize::from(self.max_workspaces)
            || self
                .workspace_profiles
                .iter()
                .any(|reference| reference.revision == 0)
        {
            return Err(ProfileError::new(
                "invalid_workspace_profile_reference",
                "workspace_profiles",
            ));
        }
        validate_commands(&self.software_install_commands, 32)?;
        if self.environment_variables.len() > 64 {
            return Err(ProfileError::new(
                "too_many_environment_variables",
                "environment_variables",
            ));
        }
        self.environment_variables
            .iter()
            .try_for_each(EnvironmentVariable::validate)?;
        let names: BTreeSet<_> = self
            .environment_variables
            .iter()
            .map(|variable| variable.name.as_str())
            .collect();
        if names.len() != self.environment_variables.len() {
            return Err(ProfileError::new(
                "duplicate_environment_variable",
                "environment_variables",
            ));
        }
        Ok(())
    }

    pub fn validate_public_values(&self, secret_patterns: &[String]) -> Result<(), ProfileError> {
        for variable in &self.environment_variables {
            if secret_patterns
                .iter()
                .filter(|pattern| !pattern.is_empty())
                .any(|pattern| variable.value.contains(pattern))
            {
                return Err(ProfileError::new(
                    "secret_like_environment_value",
                    "environment_variables",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfile {
    pub id: SandboxProfileId,
    pub owner: SubjectId,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibility,
    pub draft: SandboxProfileContent,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl SandboxProfile {
    pub fn validate(&self, bounds: ProfileCapacityBounds) -> Result<(), ProfileError> {
        validate_metadata(&self.name, &self.description)?;
        self.draft.validate(bounds)?;
        if self.visibility == ProfileVisibility::Public {
            self.draft.validate_public_values(&[])?;
        }
        Ok(())
    }

    pub fn publish(
        &self,
        revision: u32,
        published_at: String,
        bounds: ProfileCapacityBounds,
        secret_patterns: &[String],
    ) -> Result<SandboxProfileRevision, ProfileError> {
        self.validate(bounds)?;
        if self.visibility == ProfileVisibility::Public {
            self.draft.validate_public_values(secret_patterns)?;
        }
        Ok(SandboxProfileRevision {
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
pub struct SandboxProfileRevision {
    pub profile_id: SandboxProfileId,
    pub revision: u32,
    pub content: SandboxProfileContent,
    pub digest: String,
    pub published_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileSnapshot {
    pub id: SandboxProfileId,
    pub revision: u32,
    pub digest: String,
    pub content: SandboxProfileContent,
}

impl From<&SandboxProfileRevision> for SandboxProfileSnapshot {
    fn from(revision: &SandboxProfileRevision) -> Self {
        Self {
            id: revision.profile_id.clone(),
            revision: revision.revision,
            digest: revision.digest.clone(),
            content: revision.content.clone(),
        }
    }
}
