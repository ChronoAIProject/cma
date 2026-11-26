//! Atomic ordinary and initial publication for the in-memory profile store.
use crate::profiles::references::{
    reference_is_readable, verify_agent_revision, verify_workspace_revision,
};

use super::*;

impl InMemoryProfileStore {
    pub(super) async fn publish_agent_with_initial(
        &self,
        id: &AgentProfileId,
        expected: u64,
        published_at: String,
        initial: Option<&AgentProfile>,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        let mut state = self.lock();
        let profile = state.agents.get(id).ok_or(ProfileStoreError::NotFound)?;
        if let Some(expected) = initial {
            if profile != expected {
                return Err(ProfileStoreError::Conflict);
            }
            if let Some(revision) = state.agent_revisions.get(&(id.clone(), 1)) {
                if revision.profile_id != *id
                    || revision.revision != 1
                    || revision.content != expected.draft
                    || revision.digest != crate::profiles::content_digest(&expected.draft)?
                {
                    return Err(ProfileStoreError::Conflict);
                }
                return Ok(revision.clone());
            }
        }
        if profile.draft_version != expected || profile.retired_at.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let number = next_revision(&state.agent_revisions, id)?;
        if initial.is_some() && number != 1 {
            return Err(ProfileStoreError::Conflict);
        }
        let revision = profile.publish(number, published_at)?;
        state
            .agent_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }
}

impl InMemoryProfileStore {
    pub(super) async fn publish_workspace_with_initial(
        &self,
        id: &WorkspaceProfileId,
        expected: u64,
        published_at: String,
        initial: Option<&WorkspaceProfile>,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        let mut state = self.lock();
        let profile = state
            .workspaces
            .get(id)
            .ok_or(ProfileStoreError::NotFound)?;
        if let Some(expected) = initial {
            if profile != expected {
                return Err(ProfileStoreError::Conflict);
            }
            if let Some(revision) = state.workspace_revisions.get(&(id.clone(), 1)) {
                if revision.profile_id != *id
                    || revision.revision != 1
                    || revision.content != expected.draft
                    || revision.digest != crate::profiles::content_digest(&expected.draft)?
                {
                    return Err(ProfileStoreError::Conflict);
                }
                return Ok(revision.clone());
            }
        }
        if profile.draft_version != expected || profile.retired_at.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let inherited = super::reference_memory::workspace_authority(&state, profile)?;
        for reference in &profile.draft.agent_profiles {
            let revision = state
                .agent_revisions
                .get(&(reference.id.clone(), reference.revision))
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            verify_agent_revision(reference, revision)?;
            let referenced = state
                .agents
                .get(&revision.profile_id)
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !reference_is_readable(
                &profile.owner,
                &referenced.owner,
                referenced.visibility,
                referenced.retired_at.is_some(),
                inherited.contains_agent(reference),
            ) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
        }
        let number = next_revision(&state.workspace_revisions, id)?;
        if initial.is_some() && number != 1 {
            return Err(ProfileStoreError::Conflict);
        }
        let revision = profile.publish(number, published_at)?;
        state
            .workspace_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }
}

impl InMemoryProfileStore {
    pub(super) async fn publish_sandbox_with_initial(
        &self,
        id: &SandboxProfileId,
        expected: u64,
        published_at: String,
        initial: Option<&SandboxProfile>,
        bounds: ProfileCapacityBounds,
        scanner: &dyn crate::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        let (profile, number) = {
            let state = self.lock();
            let profile = state.sandboxes.get(id).ok_or(ProfileStoreError::NotFound)?;
            if let Some(expected) = initial {
                if profile != expected {
                    return Err(ProfileStoreError::Conflict);
                }
                if let Some(revision) = state.sandbox_revisions.get(&(id.clone(), 1)) {
                    if revision.profile_id != *id
                        || revision.revision != 1
                        || revision.content != expected.draft
                        || revision.digest != crate::profiles::content_digest(&expected.draft)?
                    {
                        return Err(ProfileStoreError::Conflict);
                    }
                    return Ok(revision.clone());
                }
            }
            if profile.draft_version != expected || profile.retired_at.is_some() {
                return Err(ProfileStoreError::Conflict);
            }
            (
                profile.clone(),
                next_revision(&state.sandbox_revisions, id)?,
            )
        };
        if initial.is_some() && number != 1 {
            return Err(ProfileStoreError::Conflict);
        }
        let revision = profile
            .publish(number, published_at, bounds, scanner)
            .await?;
        let mut state = self.lock();
        if initial.is_some() && state.sandboxes.get(id) == Some(&profile) {
            if let Some(stored) = state.sandbox_revisions.get(&(id.clone(), 1)) {
                if stored.profile_id != *id
                    || stored.revision != 1
                    || stored.content != profile.draft
                    || stored.digest != crate::profiles::content_digest(&profile.draft)?
                {
                    return Err(ProfileStoreError::Conflict);
                }
                return Ok(stored.clone());
            }
        }
        if state.sandboxes.get(id) != Some(&profile)
            || next_revision(&state.sandbox_revisions, id)? != number
        {
            return Err(ProfileStoreError::Conflict);
        }
        let mut workspace_names = BTreeSet::new();
        let inherited = super::reference_memory::sandbox_authority(&state, &profile)?;
        for reference in &profile.draft.workspace_profiles {
            let revision = state
                .workspace_revisions
                .get(&(reference.id.clone(), reference.revision))
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            verify_workspace_revision(reference, revision)?;
            let referenced = state
                .workspaces
                .get(&revision.profile_id)
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !reference_is_readable(
                &profile.owner,
                &referenced.owner,
                referenced.visibility,
                referenced.retired_at.is_some(),
                inherited.contains_workspace(reference),
            ) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
            if !workspace_names.insert(revision.content.name.as_str()) {
                return Err(
                    ProfileError::new("duplicate_workspace_name", "workspace_profiles").into(),
                );
            }
        }
        state
            .sandbox_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }
}
