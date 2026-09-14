use crate::ids::SubjectId;
use crate::profiles::store::{ProfileStoreError, WorkspaceProfileStore};
use crate::profiles::{
    content_digest, AgentProfile, AgentProfileRef, AgentProfileRevision, SandboxProfile,
    SandboxProfileRef, SandboxProfileRevision, WorkspaceProfile, WorkspaceProfileRef,
    WorkspaceProfileRevision,
};

pub(super) fn validate_agent_profile(
    reference: &AgentProfileRef,
    profile: &AgentProfile,
) -> Result<(), ProfileStoreError> {
    if profile.id != reference.id {
        return Err(corrupt_dependency(
            "agent profile aggregate identity does not match",
        ));
    }
    Ok(())
}

pub(super) fn validate_agent_revision(
    reference: &AgentProfileRef,
    revision: &AgentProfileRevision,
) -> Result<(), ProfileStoreError> {
    let digest = content_digest(&revision.content)
        .map_err(|error| corrupt_dependency(&error.to_string()))?;
    if revision.profile_id != reference.id
        || revision.revision != reference.revision
        || revision.digest != digest
    {
        return Err(corrupt_dependency(
            "agent profile revision identity or digest does not match",
        ));
    }
    revision.content.validate().map_err(|error| {
        corrupt_dependency(&format!(
            "agent profile revision content is invalid: {error}"
        ))
    })?;
    Ok(())
}

pub(super) fn validate_sandbox_profile(
    reference: &SandboxProfileRef,
    profile: &SandboxProfile,
) -> Result<(), ProfileStoreError> {
    if profile.id != reference.id {
        return Err(corrupt_dependency(
            "sandbox profile aggregate identity does not match",
        ));
    }
    Ok(())
}

pub(super) fn validate_sandbox_revision(
    reference: &SandboxProfileRef,
    revision: &SandboxProfileRevision,
) -> Result<(), ProfileStoreError> {
    let digest = content_digest(&revision.content)
        .map_err(|error| corrupt_dependency(&error.to_string()))?;
    if revision.profile_id != reference.id
        || revision.revision != reference.revision
        || revision.digest != digest
    {
        return Err(corrupt_dependency(
            "sandbox profile revision identity or digest does not match",
        ));
    }
    Ok(())
}

pub(super) fn validate_workspace_profile(
    reference: &WorkspaceProfileRef,
    profile: &WorkspaceProfile,
) -> Result<(), ProfileStoreError> {
    if profile.id != reference.id {
        return Err(corrupt_dependency(
            "workspace profile aggregate identity does not match",
        ));
    }
    Ok(())
}

pub(super) fn validate_workspace_revision(
    reference: &WorkspaceProfileRef,
    revision: &WorkspaceProfileRevision,
) -> Result<(), ProfileStoreError> {
    let digest = content_digest(&revision.content)
        .map_err(|error| corrupt_dependency(&error.to_string()))?;
    if revision.profile_id != reference.id
        || revision.revision != reference.revision
        || revision.digest != digest
    {
        return Err(corrupt_dependency(
            "workspace profile revision identity or digest does not match",
        ));
    }
    revision.content.validate().map_err(|error| {
        corrupt_dependency(&format!(
            "workspace profile revision content is invalid: {error}"
        ))
    })?;
    Ok(())
}

pub(crate) async fn foreign_workspace_setups(
    profiles: &dyn WorkspaceProfileStore,
    references: &[WorkspaceProfileRef],
    consumer: &SubjectId,
) -> Result<Vec<WorkspaceProfileRevision>, ProfileStoreError> {
    let mut unsafe_revisions = Vec::new();
    for reference in references {
        let profile = profiles
            .workspace_profile(&reference.id)
            .await?
            .ok_or_else(|| corrupt_dependency("workspace profile is missing"))?;
        validate_workspace_profile(reference, &profile)?;
        let revision = profiles
            .workspace_profile_revision(&reference.id, reference.revision)
            .await?
            .ok_or_else(|| corrupt_dependency("workspace profile revision is missing"))?;
        validate_workspace_revision(reference, &revision)?;
        if profile.owner != *consumer && !revision.content.setup_commands.is_empty() {
            unsafe_revisions.push(revision);
        }
    }
    Ok(unsafe_revisions)
}

fn corrupt_dependency(detail: &str) -> ProfileStoreError {
    ProfileStoreError::Corrupt(format!("referenced {detail}"))
}
