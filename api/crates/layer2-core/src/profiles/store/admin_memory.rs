use async_trait::async_trait;

use super::*;
use crate::profiles::{
    ProfileAdminQuery, ProfileAdminStore, ProfileAdminSummary, ProfileKind, ProfileLocator,
};

fn matches(
    query: &ProfileAdminQuery,
    kind: ProfileKind,
    owner: &SubjectId,
    visibility: ProfileVisibility,
) -> bool {
    query.kind.is_none_or(|filter| filter == kind)
        && query.owner.as_ref().is_none_or(|filter| filter == owner)
        && query.visibility.is_none_or(|filter| filter == visibility)
}

#[async_trait]
impl ProfileAdminStore for InMemoryProfileStore {
    async fn list_profiles(
        &self,
        query: &ProfileAdminQuery,
    ) -> Result<ProfilePage<ProfileAdminSummary>, ProfileStoreError> {
        let state = self.lock();
        let mut values = Vec::new();
        for profile in state.agents.values() {
            if !profile.internal
                && matches(
                    query,
                    ProfileKind::Agent,
                    &profile.owner,
                    profile.visibility,
                )
            {
                values.push((profile.id.to_string(), ProfileAdminSummary::from(profile)));
            }
        }
        for profile in state.workspaces.values() {
            if !profile.internal
                && matches(
                    query,
                    ProfileKind::Workspace,
                    &profile.owner,
                    profile.visibility,
                )
            {
                values.push((profile.id.to_string(), ProfileAdminSummary::from(profile)));
            }
        }
        for profile in state.sandboxes.values() {
            if !profile.internal
                && matches(
                    query,
                    ProfileKind::Sandbox,
                    &profile.owner,
                    profile.visibility,
                )
            {
                values.push((profile.id.to_string(), ProfileAdminSummary::from(profile)));
            }
        }
        profile_page(values, &query.page)
    }

    async fn retire_profile(
        &self,
        locator: &ProfileLocator,
        retired_at: String,
    ) -> Result<ProfileAdminSummary, ProfileStoreError> {
        let protected = match locator {
            ProfileLocator::Agent(id) => id.as_str() == crate::DEFAULT_AGENT_PROFILE_ID,
            ProfileLocator::Workspace(id) => id.as_str() == crate::DEFAULT_WORKSPACE_PROFILE_ID,
            ProfileLocator::Sandbox(id) => id.as_str() == crate::DEFAULT_SANDBOX_PROFILE_ID,
        };
        if protected {
            return Err(ProfileStoreError::Protected);
        }
        let mut state = self.lock();
        match locator {
            ProfileLocator::Agent(id) => {
                let profile = state
                    .agents
                    .get_mut(id)
                    .filter(|profile| !profile.internal)
                    .ok_or(ProfileStoreError::NotFound)?;
                if profile.retired_at.is_none() {
                    profile.updated_at = retired_at.clone();
                    profile.retired_at = Some(retired_at);
                }
                Ok(ProfileAdminSummary::from(&*profile))
            }
            ProfileLocator::Workspace(id) => {
                let profile = state
                    .workspaces
                    .get_mut(id)
                    .filter(|profile| !profile.internal)
                    .ok_or(ProfileStoreError::NotFound)?;
                if profile.retired_at.is_none() {
                    profile.updated_at = retired_at.clone();
                    profile.retired_at = Some(retired_at);
                }
                Ok(ProfileAdminSummary::from(&*profile))
            }
            ProfileLocator::Sandbox(id) => {
                let profile = state
                    .sandboxes
                    .get_mut(id)
                    .filter(|profile| !profile.internal)
                    .ok_or(ProfileStoreError::NotFound)?;
                if profile.retired_at.is_none() {
                    profile.updated_at = retired_at.clone();
                    profile.retired_at = Some(retired_at);
                }
                Ok(ProfileAdminSummary::from(&*profile))
            }
        }
    }
}
