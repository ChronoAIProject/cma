use std::collections::BTreeSet;

pub use layer3_adapter::AgentProvider;
use serde::{Deserialize, Serialize};

use crate::ids::{SandboxProfileId, ServiceId, SubjectId};

use super::common::{
    validate_metadata, ProfileError, ProfileVisibility, SandboxProfileRef, WorkspaceProfileRef,
};
use super::content_digest;
use super::validation::validate_commands;
use super::{PublicValueScanError, PublicValueScanner};

const RESERVED_ENVIRONMENT_NAMES: &[&str] = &["PATH", "HOME", "NYXID_URL", "NYXID_ACCESS_TOKEN"];

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
    fn validate(&self, provider: AgentProvider) -> Result<(), ProfileError> {
        let mut chars = self.name.chars();
        if !chars
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
            || !chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            || RESERVED_ENVIRONMENT_NAMES.contains(&self.name.as_str())
            || provider
                .reserved_environment_names()
                .contains(&self.name.as_str())
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
    /// Immutable OCI image selected by Agent Manager.  A missing value uses
    /// the deployment's system default image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_image: Option<String>,
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
        if self
            .runtime_image
            .as_deref()
            .is_some_and(|image| !valid_runtime_image(image))
        {
            return Err(ProfileError::new("invalid_runtime_image", "runtime_image"));
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
            .try_for_each(|variable| variable.validate(self.agent_provider))?;
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

    pub async fn validate_public_values(
        &self,
        scanner: &dyn PublicValueScanner,
    ) -> Result<(), ProfileError> {
        match scanner.scan(&self.environment_variables).await {
            Ok(()) => Ok(()),
            Err(PublicValueScanError::Secret { variable })
                if self
                    .environment_variables
                    .iter()
                    .any(|value| value.name == variable) =>
            {
                Err(ProfileError::environment_secret(&variable))
            }
            Err(_) => Err(ProfileError::new(
                "public_value_scanner_unavailable",
                "environment_variables",
            )),
        }
    }
}

pub(crate) fn valid_runtime_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    !name.is_empty()
        && name.len() <= 2_048
        && !name.contains('@')
        && !name.contains("://")
        && !name.starts_with(['.', '-', '_', '/', ':'])
        && !name.ends_with(['.', '-', '_', '/', ':'])
        && !name.contains("//")
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/' | b':')
        })
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfile {
    /// Immutable service-owned snapshot, excluded from profile management.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    #[serde(default)]
    pub availability: super::SandboxProfileAvailability,
    pub id: SandboxProfileId,
    pub owner: SubjectId,
    pub name: String,
    pub description: String,
    pub visibility: ProfileVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<SandboxProfileRef>,
    #[serde(default)]
    pub uses_count: u64,
    pub draft: SandboxProfileContent,
    pub draft_version: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl SandboxProfile {
    pub fn validate(&self, bounds: ProfileCapacityBounds) -> Result<(), ProfileError> {
        self.availability.validate(self.visibility)?;
        if self.internal && self.visibility != ProfileVisibility::Private {
            return Err(ProfileError::new(
                "internal_profile_must_be_private",
                "visibility",
            ));
        }
        validate_metadata(&self.name, &self.description)?;
        self.draft.validate(bounds)?;
        Ok(())
    }

    pub async fn publish(
        &self,
        revision: u32,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileError> {
        self.validate(bounds)?;
        if self.visibility == ProfileVisibility::Public {
            self.draft.validate_public_values(scanner).await?;
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
    /// Per-Sandbox automatic-pause exception requiring administrator approval.
    /// Owners may relinquish it. `None` preserves historical profile
    /// availability until the platform auto-pause policy is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always_on: Option<bool>,
    pub content: SandboxProfileContent,
}

impl From<&SandboxProfileRevision> for SandboxProfileSnapshot {
    fn from(revision: &SandboxProfileRevision) -> Self {
        Self {
            id: revision.profile_id.clone(),
            revision: revision.revision,
            digest: revision.digest.clone(),
            always_on: None,
            content: revision.content.clone(),
        }
    }
}
