use crate::profiles::references::{
    sandbox_evidence_matches, workspace_evidence_matches, InheritedReferences,
};

use super::*;

pub(super) fn workspace_authority(
    state: &State,
    parent: &WorkspaceProfile,
) -> Result<InheritedReferences, ProfileStoreError> {
    let mut inherited = InheritedReferences::default();
    if parent.forked_from.as_ref().is_some_and(|source| {
        !state
            .workspace_revisions
            .contains_key(&(source.id.clone(), source.revision))
    }) {
        return Err(ProfileStoreError::ReferenceUnavailable);
    }
    for ((id, number), revision) in &state.workspace_revisions {
        if workspace_evidence_matches(parent, id, *number) {
            if &revision.profile_id != id || revision.revision != *number {
                return Err(ProfileStoreError::Corrupt(
                    "workspace evidence identity disagrees".into(),
                ));
            }
            inherited.inherit_workspace(parent, revision)?;
        }
    }
    Ok(inherited)
}

pub(super) fn sandbox_authority(
    state: &State,
    parent: &SandboxProfile,
) -> Result<InheritedReferences, ProfileStoreError> {
    let mut inherited = InheritedReferences::default();
    if parent.forked_from.as_ref().is_some_and(|source| {
        !state
            .sandbox_revisions
            .contains_key(&(source.id.clone(), source.revision))
    }) {
        return Err(ProfileStoreError::ReferenceUnavailable);
    }
    for ((id, number), revision) in &state.sandbox_revisions {
        if sandbox_evidence_matches(parent, id, *number) {
            if &revision.profile_id != id || revision.revision != *number {
                return Err(ProfileStoreError::Corrupt(
                    "sandbox evidence identity disagrees".into(),
                ));
            }
            inherited.inherit_sandbox(parent, revision)?;
        }
    }
    Ok(inherited)
}
