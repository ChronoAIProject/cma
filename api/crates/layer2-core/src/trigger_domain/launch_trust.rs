use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::profiles::{
    EnvironmentVariable, ProfileVisibility, SandboxProfileStore, WorkspaceProfileRef,
    WorkspaceProfileStore,
};
use crate::SubjectId;

use super::requirements::valid_digest;
use super::{LaunchResolution, LaunchServiceError, ResolvedTriggerProfiles, TriggerError};

const TRUST_MANIFEST_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchTrustResource {
    Sandbox,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchTrustDisclosure {
    pub resource: LaunchTrustResource,
    pub name: String,
    pub commands_digest: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub environment_variables: Vec<EnvironmentVariable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchTrustManifest {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_digest: Option<String>,
    #[serde(default)]
    pub disclosures: Vec<LaunchTrustDisclosure>,
}

impl LaunchTrustManifest {
    pub fn none() -> Self {
        Self {
            version: TRUST_MANIFEST_VERSION,
            confirmation_digest: None,
            sandbox_digest: None,
            workspace_digest: None,
            disclosures: Vec::new(),
        }
    }

    pub(super) async fn resolve(
        sandbox_profiles: &dyn SandboxProfileStore,
        workspace_profiles: &dyn WorkspaceProfileStore,
        profiles: &ResolvedTriggerProfiles,
        resolution: &LaunchResolution,
        consumer: &SubjectId,
    ) -> Result<Self, LaunchServiceError> {
        Self::resolve_version(
            TRUST_MANIFEST_VERSION,
            sandbox_profiles,
            workspace_profiles,
            profiles,
            resolution,
            consumer,
        )
        .await
    }

    pub(super) async fn resolve_version(
        version: u8,
        sandbox_profiles: &dyn SandboxProfileStore,
        workspace_profiles: &dyn WorkspaceProfileStore,
        profiles: &ResolvedTriggerProfiles,
        resolution: &LaunchResolution,
        consumer: &SubjectId,
    ) -> Result<Self, LaunchServiceError> {
        let (sandbox_digest, mut disclosures) = if resolution.sandbox.is_pending() {
            sandbox_trust(sandbox_profiles, workspace_profiles, profiles, consumer).await?
        } else {
            (None, Vec::new())
        };
        let (workspace_digest, workspace_disclosure) =
            if creates_target_workspace(profiles, resolution) {
                workspace_trust(workspace_profiles, profiles, consumer).await?
            } else {
                (None, None)
            };
        disclosures.extend(workspace_disclosure);
        let confirmation_digest = manifest_digest(
            version,
            sandbox_digest.as_deref(),
            workspace_digest.as_deref(),
        )
        .ok_or_else(|| corrupt("Launch trust manifest version is unsupported"))?;
        let manifest = Self {
            version,
            confirmation_digest,
            sandbox_digest,
            workspace_digest,
            disclosures,
        };
        manifest
            .validate()
            .map_err(|error| LaunchServiceError::Corrupt(error.to_string()))?;
        Ok(manifest)
    }

    pub(super) fn matches_frozen(&self, resolved: &Self) -> bool {
        self.version == resolved.version
            && self.confirmation_digest == resolved.confirmation_digest
            && self.sandbox_digest == resolved.sandbox_digest
            && self.workspace_digest == resolved.workspace_digest
            && self.disclosures.len() == resolved.disclosures.len()
            && self
                .disclosures
                .iter()
                .zip(&resolved.disclosures)
                .all(|(stored, current)| {
                    stored.resource == current.resource
                        && stored.commands_digest == current.commands_digest
                        && stored.commands == current.commands
                        && stored.environment_variables == current.environment_variables
                })
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        let expected = manifest_digest(
            self.version,
            self.sandbox_digest.as_deref(),
            self.workspace_digest.as_deref(),
        );
        if expected.as_ref() != Some(&self.confirmation_digest)
            || self
                .confirmation_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || self
                .sandbox_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || self
                .workspace_digest
                .as_deref()
                .is_some_and(|digest| !valid_digest(digest))
            || (self.confirmation_digest.is_some() == self.disclosures.is_empty())
            || invalid_manifest_shape(self)
            || self.disclosures.iter().any(invalid_disclosure)
        {
            return Err(TriggerError::new("invalid_launch_trust_manifest", "trust"));
        }
        Ok(())
    }
}

async fn sandbox_trust(
    sandbox_profiles: &dyn SandboxProfileStore,
    workspace_profiles: &dyn WorkspaceProfileStore,
    profiles: &ResolvedTriggerProfiles,
    consumer: &SubjectId,
) -> Result<(Option<String>, Vec<LaunchTrustDisclosure>), LaunchServiceError> {
    let profile = sandbox_profiles
        .sandbox_profile(&profiles.sandbox.profile_id)
        .await
        .map_err(super::launch_service_support::profile_error)?
        .ok_or_else(|| corrupt("Sandbox Profile is missing"))?;
    if profile.id != profiles.sandbox.profile_id {
        return Err(corrupt("Sandbox Profile identity does not match"));
    }
    let outer_requires_trust = profile.owner != *consumer
        && (!profiles
            .sandbox
            .content
            .software_install_commands
            .is_empty()
            || !profiles.sandbox.content.environment_variables.is_empty());
    let nested = crate::sandbox::foreign_workspace_setups(
        workspace_profiles,
        &profiles.sandbox.content.workspace_profiles,
        consumer,
    )
    .await
    .map_err(super::launch_service_support::profile_error)?;
    let mut disclosures = Vec::new();
    if outer_requires_trust {
        disclosures.push(LaunchTrustDisclosure {
            resource: LaunchTrustResource::Sandbox,
            name: disclosure_name(
                &profile.name,
                &profile.description,
                &profile.owner,
                profile.visibility,
                profile.retired_at.as_deref(),
                profile.forked_from.is_some(),
                LaunchTrustResource::Sandbox,
            ),
            commands_digest: profiles.sandbox.digest.clone(),
            commands: profiles.sandbox.content.software_install_commands.clone(),
            environment_variables: profiles.sandbox.content.environment_variables.clone(),
        });
    }
    for revision in nested {
        let name = revision
            .content
            .workspace_name()
            .map_err(|error| corrupt(&error.to_string()))?;
        disclosures.push(LaunchTrustDisclosure {
            resource: LaunchTrustResource::Workspace,
            name,
            commands_digest: revision.digest,
            commands: revision.content.setup_commands,
            environment_variables: Vec::new(),
        });
    }
    let digest = (!disclosures.is_empty()).then(|| profiles.sandbox.digest.clone());
    Ok((digest, disclosures))
}

fn creates_target_workspace(
    profiles: &ResolvedTriggerProfiles,
    resolution: &LaunchResolution,
) -> bool {
    resolution.workspace.is_pending()
        && (!resolution.sandbox.is_pending()
            || !contains_profile_id(
                &profiles.sandbox.content.workspace_profiles,
                &profiles.workspace.profile_id,
            ))
}

fn contains_profile_id(references: &[WorkspaceProfileRef], id: &crate::WorkspaceProfileId) -> bool {
    references.iter().any(|reference| &reference.id == id)
}

async fn workspace_trust(
    workspace_profiles: &dyn WorkspaceProfileStore,
    profiles: &ResolvedTriggerProfiles,
    consumer: &SubjectId,
) -> Result<(Option<String>, Option<LaunchTrustDisclosure>), LaunchServiceError> {
    if profiles.workspace.content.setup_commands.is_empty() {
        return Ok((None, None));
    }
    let profile = workspace_profiles
        .workspace_profile(&profiles.workspace.profile_id)
        .await
        .map_err(super::launch_service_support::profile_error)?
        .ok_or_else(|| corrupt("Workspace Profile is missing"))?;
    if profile.id != profiles.workspace.profile_id {
        return Err(corrupt("Workspace Profile identity does not match"));
    }
    if profile.owner == *consumer {
        return Ok((None, None));
    }
    Ok((
        Some(profiles.workspace.digest.clone()),
        Some(LaunchTrustDisclosure {
            resource: LaunchTrustResource::Workspace,
            name: disclosure_name(
                &profile.name,
                &profile.description,
                &profile.owner,
                profile.visibility,
                profile.retired_at.as_deref(),
                profile.forked_from.is_some(),
                LaunchTrustResource::Workspace,
            ),
            commands_digest: profiles.workspace.digest.clone(),
            commands: profiles.workspace.content.setup_commands.clone(),
            environment_variables: Vec::new(),
        }),
    ))
}

fn disclosure_name(
    name: &str,
    description: &str,
    owner: &SubjectId,
    visibility: ProfileVisibility,
    retired_at: Option<&str>,
    has_fork_attribution: bool,
    resource: LaunchTrustResource,
) -> String {
    if name.is_empty()
        && description.is_empty()
        && owner.as_str() == crate::DELETED_ACCOUNT_SUBJECT
        && visibility == ProfileVisibility::Private
        && retired_at.is_some_and(|at| crate::seconds_from_rfc3339(at).is_some())
        && !has_fork_attribution
    {
        // Display text never restores authoring metadata or grants pin authority.
        return match resource {
            LaunchTrustResource::Sandbox => "Sandbox",
            LaunchTrustResource::Workspace => "Workspace",
        }
        .into();
    }
    name.to_owned()
}

fn manifest_digest(
    version: u8,
    sandbox: Option<&str>,
    workspace: Option<&str>,
) -> Option<Option<String>> {
    if version != 1 {
        return None;
    }
    Some(manifest_digest_v1(sandbox, workspace))
}

fn manifest_digest_v1(sandbox: Option<&str>, workspace: Option<&str>) -> Option<String> {
    match (sandbox, workspace) {
        (None, None) => None,
        (Some(digest), None) | (None, Some(digest)) => Some(digest.to_string()),
        (Some(sandbox), Some(workspace)) => {
            let mut digest = Sha256::new();
            digest.update(b"cma.trigger-launch.trust-manifest.v1\0");
            digest.update(b"sandbox\0");
            digest.update(sandbox.as_bytes());
            digest.update(b"\0workspace\0");
            digest.update(workspace.as_bytes());
            Some(format!("{:x}", digest.finalize()))
        }
    }
}

fn invalid_manifest_shape(manifest: &LaunchTrustManifest) -> bool {
    let disclosures = manifest.disclosures.as_slice();
    match (
        manifest.sandbox_digest.as_deref(),
        manifest.workspace_digest.as_deref(),
    ) {
        (None, None) => !disclosures.is_empty(),
        (None, Some(workspace)) => {
            disclosures.len() != 1
                || disclosures[0].resource != LaunchTrustResource::Workspace
                || disclosures[0].commands_digest != workspace
        }
        (Some(sandbox), workspace) => {
            let Some(sandbox_disclosure_count) = disclosures
                .len()
                .checked_sub(usize::from(workspace.is_some()))
            else {
                return true;
            };
            sandbox_disclosure_count == 0
                || disclosures[..sandbox_disclosure_count]
                    .iter()
                    .skip(1)
                    .any(|disclosure| disclosure.resource == LaunchTrustResource::Sandbox)
                || (disclosures[0].resource == LaunchTrustResource::Sandbox
                    && disclosures[0].commands_digest != sandbox)
                || workspace.is_some_and(|digest| {
                    let target = disclosures.last().expect("a target disclosure exists");
                    target.resource != LaunchTrustResource::Workspace
                        || target.commands_digest != digest
                })
        }
    }
}

fn invalid_disclosure(disclosure: &LaunchTrustDisclosure) -> bool {
    disclosure.name.trim().is_empty()
        || disclosure.name.len() > 128
        || !valid_digest(&disclosure.commands_digest)
        || (disclosure.commands.is_empty() && disclosure.environment_variables.is_empty())
        || (disclosure.resource == LaunchTrustResource::Workspace
            && !disclosure.environment_variables.is_empty())
}

fn corrupt(detail: &str) -> LaunchServiceError {
    LaunchServiceError::Corrupt(detail.into())
}

#[cfg(test)]
#[path = "launch_trust_retained_label_tests.rs"]
mod retained_label_tests;
