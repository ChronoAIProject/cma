use std::collections::BTreeSet;

use super::*;
use crate::profiles::{
    AgentProfileStore, ProfilePageRequest, SandboxProfileStore, WorkspaceProfileStore,
};

#[derive(Debug)]
pub struct ReferenceValidationError {
    pub index: Option<usize>,
    pub error: ProfileStoreError,
}

impl From<ProfileStoreError> for ReferenceValidationError {
    fn from(error: ProfileStoreError) -> Self {
        Self { index: None, error }
    }
}

pub async fn validate_agent_references(
    agents: &dyn AgentProfileStore,
    workspaces: &dyn WorkspaceProfileStore,
    owner: &SubjectId,
    parent: Option<&WorkspaceProfileId>,
    references: &[AgentProfileRef],
) -> Result<(), ReferenceValidationError> {
    let inherited = workspace_authority(workspaces, owner, parent).await?;
    for (index, reference) in references.iter().enumerate() {
        let result = async {
            let profile = agents
                .agent_profile(&reference.id)
                .await?
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !reference_is_readable(
                owner,
                &profile.owner,
                profile.visibility,
                profile.retired_at.is_some(),
                inherited.contains_agent(reference),
            ) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
            let revision = agents
                .agent_profile_revision(&reference.id, reference.revision)
                .await?
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            verify_agent_revision(reference, &revision)
        }
        .await;
        result.map_err(|error| ReferenceValidationError {
            index: Some(index),
            error,
        })?;
    }
    Ok(())
}

pub async fn validate_workspace_references(
    workspaces: &dyn WorkspaceProfileStore,
    sandboxes: &dyn SandboxProfileStore,
    owner: &SubjectId,
    parent: Option<&SandboxProfileId>,
    references: &[WorkspaceProfileRef],
) -> Result<(), ReferenceValidationError> {
    let inherited = sandbox_authority(sandboxes, owner, parent).await?;
    for (index, reference) in references.iter().enumerate() {
        let result = async {
            let profile = workspaces
                .workspace_profile(&reference.id)
                .await?
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !reference_is_readable(
                owner,
                &profile.owner,
                profile.visibility,
                profile.retired_at.is_some(),
                inherited.contains_workspace(reference),
            ) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
            let revision = workspaces
                .workspace_profile_revision(&reference.id, reference.revision)
                .await?
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            verify_workspace_revision(reference, &revision)
        }
        .await;
        result.map_err(|error| ReferenceValidationError {
            index: Some(index),
            error,
        })?;
    }
    Ok(())
}

async fn workspace_authority(
    store: &dyn WorkspaceProfileStore,
    owner: &SubjectId,
    parent: Option<&WorkspaceProfileId>,
) -> Result<InheritedReferences, ProfileStoreError> {
    let mut inherited = InheritedReferences::default();
    let Some(id) = parent else {
        return Ok(inherited);
    };
    let parent = store
        .workspace_profile(id)
        .await?
        .filter(|parent| &parent.owner == owner)
        .ok_or(ProfileStoreError::NotFound)?;
    if let Some(source) = &parent.forked_from {
        let revision = store
            .workspace_profile_revision(&source.id, source.revision)
            .await?
            .filter(|revision| {
                revision.profile_id == source.id && revision.revision == source.revision
            })
            .ok_or(ProfileStoreError::ReferenceUnavailable)?;
        inherited.inherit_workspace(&parent, &revision)?;
    }
    let mut page = ProfilePageRequest {
        limit: 200,
        cursor: None,
    };
    let mut seen = BTreeSet::new();
    loop {
        let revisions = store.list_workspace_profile_revisions(id, &page).await?;
        for revision in revisions.items {
            inherited.inherit_workspace(&parent, &revision)?;
        }
        page.cursor = advance(revisions.next, &mut seen)?;
        if page.cursor.is_none() {
            break;
        }
    }
    Ok(inherited)
}

async fn sandbox_authority(
    store: &dyn SandboxProfileStore,
    owner: &SubjectId,
    parent: Option<&SandboxProfileId>,
) -> Result<InheritedReferences, ProfileStoreError> {
    let mut inherited = InheritedReferences::default();
    let Some(id) = parent else {
        return Ok(inherited);
    };
    let parent = store
        .sandbox_profile(id)
        .await?
        .filter(|parent| &parent.owner == owner)
        .ok_or(ProfileStoreError::NotFound)?;
    if let Some(source) = &parent.forked_from {
        let revision = store
            .sandbox_profile_revision(&source.id, source.revision)
            .await?
            .filter(|revision| {
                revision.profile_id == source.id && revision.revision == source.revision
            })
            .ok_or(ProfileStoreError::ReferenceUnavailable)?;
        inherited.inherit_sandbox(&parent, &revision)?;
    }
    let mut page = ProfilePageRequest {
        limit: 200,
        cursor: None,
    };
    let mut seen = BTreeSet::new();
    loop {
        let revisions = store.list_sandbox_profile_revisions(id, &page).await?;
        for revision in revisions.items {
            inherited.inherit_sandbox(&parent, &revision)?;
        }
        page.cursor = advance(revisions.next, &mut seen)?;
        if page.cursor.is_none() {
            break;
        }
    }
    Ok(inherited)
}

fn advance(
    next: Option<String>,
    seen: &mut BTreeSet<String>,
) -> Result<Option<String>, ProfileStoreError> {
    if next.as_ref().is_some_and(|next| !seen.insert(next.clone())) {
        return Err(ProfileStoreError::Corrupt(
            "profile revision cursor repeated".into(),
        ));
    }
    Ok(next)
}
