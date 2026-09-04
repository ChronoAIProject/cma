use async_trait::async_trait;

use super::*;

macro_rules! save_draft {
    ($state:expr, $collection:ident, $id:expr, $expected:expr, $profile:expr) => {{
        let stored = $state
            .$collection
            .get(&$id)
            .ok_or(ProfileStoreError::NotFound)?;
        if stored.draft_version != $expected || $profile.draft_version != $expected + 1 {
            return Err(ProfileStoreError::Conflict);
        }
        $profile.validate()?;
        $state.$collection.insert($id, $profile);
        Ok(())
    }};
}

macro_rules! retire_profile {
    ($state:expr, $collection:ident, $id:expr, $owner:expr, $at:expr) => {{
        let profile = $state
            .$collection
            .get_mut($id)
            .filter(|profile| &profile.owner == $owner)
            .ok_or(ProfileStoreError::NotFound)?;
        profile.retired_at = Some($at.clone());
        profile.updated_at = $at;
        Ok(())
    }};
}

#[async_trait]
impl AgentProfileStore for InMemoryProfileStore {
    async fn create_agent_profile(&self, profile: AgentProfile) -> Result<(), ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version == 0 {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state.agents.insert(profile.id.clone(), profile).is_some() {
            return Err(ProfileStoreError::Duplicate);
        }
        Ok(())
    }

    async fn save_agent_draft(
        &self,
        expected: u64,
        profile: AgentProfile,
    ) -> Result<(), ProfileStoreError> {
        let id = profile.id.clone();
        let mut state = self.lock();
        save_draft!(state, agents, id, expected, profile)
    }

    async fn publish_agent_profile(
        &self,
        id: &AgentProfileId,
        expected: u64,
        published_at: String,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        let mut state = self.lock();
        let profile = state.agents.get(id).ok_or(ProfileStoreError::NotFound)?;
        if profile.draft_version != expected || profile.retired_at.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let number = next_revision(&state.agent_revisions, id)?;
        let revision = profile.publish(number, published_at)?;
        state
            .agent_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }

    async fn agent_profile_revision(
        &self,
        id: &AgentProfileId,
        revision: u32,
    ) -> Result<Option<AgentProfileRevision>, ProfileStoreError> {
        Ok(self
            .lock()
            .agent_revisions
            .get(&(id.clone(), revision))
            .cloned())
    }

    async fn list_agent_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfile>, ProfileStoreError> {
        let values = self
            .lock()
            .agents
            .values()
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| !public_only || profile.visibility == ProfileVisibility::Public)
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn retire_agent_profile(
        &self,
        id: &AgentProfileId,
        owner: &SubjectId,
        at: String,
    ) -> Result<(), ProfileStoreError> {
        let mut state = self.lock();
        retire_profile!(state, agents, id, owner, at)
    }
}

#[async_trait]
impl WorkspaceProfileStore for InMemoryProfileStore {
    async fn create_workspace_profile(
        &self,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version == 0 {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state
            .workspaces
            .insert(profile.id.clone(), profile)
            .is_some()
        {
            return Err(ProfileStoreError::Duplicate);
        }
        Ok(())
    }

    async fn save_workspace_draft(
        &self,
        expected: u64,
        profile: WorkspaceProfile,
    ) -> Result<(), ProfileStoreError> {
        let id = profile.id.clone();
        let mut state = self.lock();
        save_draft!(state, workspaces, id, expected, profile)
    }

    async fn publish_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        expected: u64,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        let mut state = self.lock();
        let profile = state
            .workspaces
            .get(id)
            .ok_or(ProfileStoreError::NotFound)?;
        if profile.draft_version != expected || profile.retired_at.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        for reference in &profile.draft.agent_profiles {
            let revision = state
                .agent_revisions
                .get(&(reference.id.clone(), reference.revision))
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            let referenced = state
                .agents
                .get(&revision.profile_id)
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !readable(&profile.owner, &referenced.owner, referenced.visibility) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
        }
        let number = next_revision(&state.workspace_revisions, id)?;
        let revision = profile.publish(number, published_at)?;
        state
            .workspace_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }

    async fn workspace_profile_revision(
        &self,
        id: &WorkspaceProfileId,
        revision: u32,
    ) -> Result<Option<WorkspaceProfileRevision>, ProfileStoreError> {
        Ok(self
            .lock()
            .workspace_revisions
            .get(&(id.clone(), revision))
            .cloned())
    }

    async fn list_workspace_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfile>, ProfileStoreError> {
        let values = self
            .lock()
            .workspaces
            .values()
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| !public_only || profile.visibility == ProfileVisibility::Public)
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn retire_workspace_profile(
        &self,
        id: &WorkspaceProfileId,
        owner: &SubjectId,
        at: String,
    ) -> Result<(), ProfileStoreError> {
        let mut state = self.lock();
        retire_profile!(state, workspaces, id, owner, at)
    }
}

#[async_trait]
impl SandboxProfileStore for InMemoryProfileStore {
    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        if profile.draft_version == 0 {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state
            .sandboxes
            .insert(profile.id.clone(), profile)
            .is_some()
        {
            return Err(ProfileStoreError::Duplicate);
        }
        Ok(())
    }

    async fn save_sandbox_draft(
        &self,
        expected: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        let id = profile.id.clone();
        let mut state = self.lock();
        let stored = state
            .sandboxes
            .get(&id)
            .ok_or(ProfileStoreError::NotFound)?;
        if stored.draft_version != expected || profile.draft_version != expected + 1 {
            return Err(ProfileStoreError::Conflict);
        }
        state.sandboxes.insert(id, profile);
        Ok(())
    }

    async fn publish_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        expected: u64,
        published_at: String,
        bounds: ProfileCapacityBounds,
        secret_patterns: &[String],
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        let mut state = self.lock();
        let profile = state.sandboxes.get(id).ok_or(ProfileStoreError::NotFound)?;
        if profile.draft_version != expected || profile.retired_at.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let mut workspace_names = BTreeSet::new();
        for reference in &profile.draft.workspace_profiles {
            let revision = state
                .workspace_revisions
                .get(&(reference.id.clone(), reference.revision))
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            let referenced = state
                .workspaces
                .get(&revision.profile_id)
                .ok_or(ProfileStoreError::ReferenceUnavailable)?;
            if !readable(&profile.owner, &referenced.owner, referenced.visibility) {
                return Err(ProfileStoreError::ReferenceUnavailable);
            }
            if !workspace_names.insert(revision.content.name.as_str()) {
                return Err(
                    ProfileError::new("duplicate_workspace_name", "workspace_profiles").into(),
                );
            }
        }
        let number = next_revision(&state.sandbox_revisions, id)?;
        let revision = profile.publish(number, published_at, bounds, secret_patterns)?;
        state
            .sandbox_revisions
            .insert((id.clone(), number), revision.clone());
        Ok(revision)
    }

    async fn sandbox_profile_revision(
        &self,
        id: &SandboxProfileId,
        revision: u32,
    ) -> Result<Option<SandboxProfileRevision>, ProfileStoreError> {
        Ok(self
            .lock()
            .sandbox_revisions
            .get(&(id.clone(), revision))
            .cloned())
    }

    async fn list_sandbox_profiles(
        &self,
        owner: Option<&SubjectId>,
        public_only: bool,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfile>, ProfileStoreError> {
        let values = self
            .lock()
            .sandboxes
            .values()
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| !public_only || profile.visibility == ProfileVisibility::Public)
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn retire_sandbox_profile(
        &self,
        id: &SandboxProfileId,
        owner: &SubjectId,
        at: String,
    ) -> Result<(), ProfileStoreError> {
        let mut state = self.lock();
        retire_profile!(state, sandboxes, id, owner, at)
    }
}
