use async_trait::async_trait;

use super::*;

#[macro_use]
#[path = "memory_macros.rs"]
mod macros;

#[async_trait]
impl AgentProfileStore for InMemoryProfileStore {
    async fn create_agent_profile(&self, profile: AgentProfile) -> Result<(), ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version == 0 || profile.forked_from.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state.agents.contains_key(&profile.id) {
            return Err(ProfileStoreError::Duplicate);
        }
        state.agents.insert(profile.id.clone(), profile);
        Ok(())
    }

    async fn agent_profile(
        &self,
        id: &AgentProfileId,
    ) -> Result<Option<AgentProfile>, ProfileStoreError> {
        Ok(self.lock().agents.get(id).cloned())
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
        self.publish_agent_with_initial(id, expected, published_at, None)
            .await
    }

    async fn ensure_initial_agent_profile(
        &self,
        profile: &AgentProfile,
        published_at: String,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version != 1
            || profile.retired_at.is_some()
            || profile.visibility != ProfileVisibility::Private
            || profile.forked_from.is_some()
        {
            return Err(ProfileStoreError::Conflict);
        }
        self.publish_agent_with_initial(&profile.id, 1, published_at, Some(profile))
            .await
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

    async fn list_agent_profile_revisions(
        &self,
        id: &AgentProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<AgentProfileRevision>, ProfileStoreError> {
        let state = self.lock();
        if !state.agents.contains_key(id) {
            return Err(ProfileStoreError::NotFound);
        }
        revision_page(
            state
                .agent_revisions
                .iter()
                .filter(|((stored, _), _)| stored == id)
                .map(|((_, number), revision)| (*number, revision.clone()))
                .collect(),
            page,
        )
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
            .filter(|profile| !profile.internal)
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| {
                !public_only
                    || (profile.visibility == ProfileVisibility::Public
                        && profile.retired_at.is_none())
            })
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn list_agent_profile_summaries(
        &self,
        owner: &SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<AgentProfile>>, ProfileStoreError> {
        self.agent_summaries(owner, page)
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
        if profile.draft_version == 0 || profile.forked_from.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state.workspaces.contains_key(&profile.id) {
            return Err(ProfileStoreError::Duplicate);
        }
        state.workspaces.insert(profile.id.clone(), profile);
        Ok(())
    }

    async fn workspace_profile(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<Option<WorkspaceProfile>, ProfileStoreError> {
        Ok(self.lock().workspaces.get(id).cloned())
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
        self.publish_workspace_with_initial(id, expected, published_at, None)
            .await
    }

    async fn ensure_initial_workspace_profile(
        &self,
        profile: &WorkspaceProfile,
        published_at: String,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        profile.validate()?;
        if profile.draft_version != 1
            || profile.retired_at.is_some()
            || profile.visibility != ProfileVisibility::Private
            || profile.forked_from.is_some()
        {
            return Err(ProfileStoreError::Conflict);
        }
        self.publish_workspace_with_initial(&profile.id, 1, published_at, Some(profile))
            .await
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

    async fn list_workspace_profile_revisions(
        &self,
        id: &WorkspaceProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<WorkspaceProfileRevision>, ProfileStoreError> {
        let state = self.lock();
        if !state.workspaces.contains_key(id) {
            return Err(ProfileStoreError::NotFound);
        }
        revision_page(
            state
                .workspace_revisions
                .iter()
                .filter(|((stored, _), _)| stored == id)
                .map(|((_, number), revision)| (*number, revision.clone()))
                .collect(),
            page,
        )
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
            .filter(|profile| !profile.internal)
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| {
                !public_only
                    || (profile.visibility == ProfileVisibility::Public
                        && profile.retired_at.is_none())
            })
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn list_workspace_profile_summaries(
        &self,
        owner: &SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<WorkspaceProfile>>, ProfileStoreError> {
        self.workspace_summaries(owner, page)
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
    async fn sandbox_profile(
        &self,
        id: &SandboxProfileId,
    ) -> Result<Option<SandboxProfile>, ProfileStoreError> {
        Ok(self.lock().sandboxes.get(id).cloned())
    }

    async fn create_sandbox_profile(
        &self,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.availability.validate(profile.visibility)?;
        if profile.availability.warmup_enabled {
            return Err(ProfileError::new(
                "warmup_requires_published_profile",
                "availability.warmup_enabled",
            )
            .into());
        }
        if profile.draft_version == 0 || profile.forked_from.is_some() {
            return Err(ProfileStoreError::Conflict);
        }
        let mut state = self.lock();
        if state.sandboxes.contains_key(&profile.id) {
            return Err(ProfileStoreError::Duplicate);
        }
        state.sandboxes.insert(profile.id.clone(), profile);
        Ok(())
    }

    async fn save_sandbox_draft(
        &self,
        expected: u64,
        profile: SandboxProfile,
    ) -> Result<(), ProfileStoreError> {
        profile.availability.validate(profile.visibility)?;
        let id = profile.id.clone();
        let mut state = self.lock();
        if profile.availability.warmup_enabled
            && !state
                .sandbox_revisions
                .keys()
                .any(|(profile_id, _)| profile_id == &id)
        {
            return Err(ProfileError::new(
                "warmup_requires_published_profile",
                "availability.warmup_enabled",
            )
            .into());
        }
        let stored = state
            .sandboxes
            .get(&id)
            .ok_or(ProfileStoreError::NotFound)?;
        let next_version = expected.checked_add(1).ok_or(ProfileStoreError::Conflict)?;
        if stored.draft_version != expected
            || profile.draft_version != next_version
            || stored.internal != profile.internal
            || stored.owner != profile.owner
            || stored.visibility != profile.visibility
            || stored.forked_from != profile.forked_from
            || stored.uses_count != profile.uses_count
            || stored.created_at != profile.created_at
            || stored.retired_at != profile.retired_at
        {
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
        scanner: &dyn crate::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        self.publish_sandbox_with_initial(id, expected, published_at, None, bounds, scanner)
            .await
    }

    async fn ensure_initial_sandbox_profile(
        &self,
        profile: &SandboxProfile,
        published_at: String,
        bounds: ProfileCapacityBounds,
        scanner: &dyn crate::profiles::PublicValueScanner,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        profile.validate(bounds)?;
        if profile.draft_version != 1
            || profile.retired_at.is_some()
            || profile.visibility != ProfileVisibility::Private
            || profile.forked_from.is_some()
        {
            return Err(ProfileStoreError::Conflict);
        }
        self.publish_sandbox_with_initial(
            &profile.id,
            1,
            published_at,
            Some(profile),
            bounds,
            scanner,
        )
        .await
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

    async fn list_sandbox_profile_revisions(
        &self,
        id: &SandboxProfileId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<SandboxProfileRevision>, ProfileStoreError> {
        let state = self.lock();
        if !state.sandboxes.contains_key(id) {
            return Err(ProfileStoreError::NotFound);
        }
        revision_page(
            state
                .sandbox_revisions
                .iter()
                .filter(|((stored, _), _)| stored == id)
                .map(|((_, number), revision)| (*number, revision.clone()))
                .collect(),
            page,
        )
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
            .filter(|profile| !profile.internal)
            .filter(|profile| owner.is_none_or(|owner| &profile.owner == owner))
            .filter(|profile| {
                !public_only
                    || (profile.visibility == ProfileVisibility::Public
                        && profile.retired_at.is_none())
            })
            .map(|profile| (profile.id.to_string(), profile.clone()))
            .collect();
        profile_page(values, page)
    }

    async fn list_sandbox_profile_summaries(
        &self,
        owner: &SubjectId,
        page: &ProfilePageRequest,
    ) -> Result<ProfilePage<ProfileSummary<SandboxProfile>>, ProfileStoreError> {
        self.sandbox_summaries(owner, page)
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
