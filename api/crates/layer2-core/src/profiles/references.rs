//! Exact-pin admission shared by profile editing and transactional publication.

use crate::{SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::{
    content_digest, AgentProfileRef, AgentProfileRevision, ProfileStoreError, ProfileVisibility,
    SandboxProfile, SandboxProfileRevision, WorkspaceProfile, WorkspaceProfileRef,
    WorkspaceProfileRevision,
};

mod validation;
pub use validation::{
    validate_agent_references, validate_workspace_references, ReferenceValidationError,
};

/// Evidence from immutable revisions, never from editable draft content.
#[derive(Default)]
pub struct InheritedReferences {
    agents: Vec<AgentProfileRef>,
    workspaces: Vec<WorkspaceProfileRef>,
}

impl InheritedReferences {
    /// `parent` must be the protected stored aggregate, not a proposed draft.
    pub fn inherit_workspace(
        &mut self,
        parent: &WorkspaceProfile,
        revision: &WorkspaceProfileRevision,
    ) -> Result<(), ProfileStoreError> {
        if !workspace_evidence_matches(parent, &revision.profile_id, revision.revision) {
            return Err(ProfileStoreError::ReferenceUnavailable);
        }
        verify_digest(&revision.content, &revision.digest)?;
        self.agents.extend(revision.content.agent_profiles.clone());
        Ok(())
    }

    /// `parent` must be the protected stored aggregate, not a proposed draft.
    pub fn inherit_sandbox(
        &mut self,
        parent: &SandboxProfile,
        revision: &SandboxProfileRevision,
    ) -> Result<(), ProfileStoreError> {
        if !sandbox_evidence_matches(parent, &revision.profile_id, revision.revision) {
            return Err(ProfileStoreError::ReferenceUnavailable);
        }
        verify_digest(&revision.content, &revision.digest)?;
        self.workspaces
            .extend(revision.content.workspace_profiles.clone());
        Ok(())
    }

    pub fn contains_agent(&self, reference: &AgentProfileRef) -> bool {
        self.agents.contains(reference)
    }

    pub fn contains_workspace(&self, reference: &WorkspaceProfileRef) -> bool {
        self.workspaces.contains(reference)
    }
}

pub fn reference_is_readable(
    owner: &SubjectId,
    child_owner: &SubjectId,
    visibility: ProfileVisibility,
    retired: bool,
    inherited: bool,
) -> bool {
    inherited || (!retired && (owner == child_owner || visibility == ProfileVisibility::Public))
}

pub fn verify_agent_revision(
    reference: &AgentProfileRef,
    revision: &AgentProfileRevision,
) -> Result<(), ProfileStoreError> {
    if reference.id != revision.profile_id || reference.revision != revision.revision {
        return Err(ProfileStoreError::ReferenceUnavailable);
    }
    verify_digest(&revision.content, &revision.digest)
}

pub fn verify_workspace_revision(
    reference: &WorkspaceProfileRef,
    revision: &WorkspaceProfileRevision,
) -> Result<(), ProfileStoreError> {
    if reference.id != revision.profile_id || reference.revision != revision.revision {
        return Err(ProfileStoreError::ReferenceUnavailable);
    }
    verify_digest(&revision.content, &revision.digest)
}

fn verify_digest<T: serde::Serialize>(content: &T, digest: &str) -> Result<(), ProfileStoreError> {
    if content_digest(content)? != digest {
        return Err(ProfileStoreError::Corrupt(
            "profile reference digest disagrees".into(),
        ));
    }
    Ok(())
}

pub fn workspace_evidence_matches(
    parent: &WorkspaceProfile,
    id: &WorkspaceProfileId,
    revision: u32,
) -> bool {
    id == &parent.id
        || parent.forked_from.as_ref() == Some(&WorkspaceProfileRef::new(id.clone(), revision))
}

pub fn sandbox_evidence_matches(
    parent: &SandboxProfile,
    id: &SandboxProfileId,
    revision: u32,
) -> bool {
    id == &parent.id
        || parent.forked_from.as_ref() == Some(&super::SandboxProfileRef::new(id.clone(), revision))
}
