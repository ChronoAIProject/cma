use super::ProviderBinding;
use crate::profiles::store::{
    AgentProfileStore, ProfileStoreError, SandboxProfileStore, WorkspaceProfileStore,
};
use crate::profiles::*;
use crate::{AgentProfileId, SandboxProfileId, WorkspaceProfileId};
use std::sync::Arc;

pub trait BindingProfileStore:
    AgentProfileStore + WorkspaceProfileStore + SandboxProfileStore
{
}
impl<T: AgentProfileStore + WorkspaceProfileStore + SandboxProfileStore> BindingProfileStore for T {}

pub(super) struct BindingProfiles {
    pub store: Arc<dyn BindingProfileStore>,
    pub bounds: ProfileCapacityBounds,
}

// Each imported content gets a private profile and always pins revision one.
// Reads verify ownership and immutable content, including after a concurrent
// publisher wins. Later dashboard edits cannot change a binding's snapshot.
impl BindingProfiles {
    pub async fn agent(
        &self,
        binding: &ProviderBinding,
    ) -> Result<AgentProfileRevision, ProfileStoreError> {
        let content = &binding.profile.agent;
        let id = AgentProfileId(format!("agp_{}", identity(binding, content)?));
        if self.store.agent_profile(&id).await?.is_none() {
            created(
                self.store
                    .create_agent_profile(AgentProfile {
                        id: id.clone(),
                        owner: binding.owner.clone(),
                        name: binding.profile.name.clone(),
                        description: "Imported external binding profile".into(),
                        visibility: ProfileVisibility::Private,
                        forked_from: None,
                        uses_count: 0,
                        draft: content.clone(),
                        draft_version: 1,
                        created_at: binding.created_at.clone(),
                        updated_at: binding.created_at.clone(),
                        retired_at: None,
                    })
                    .await,
            )?;
        }
        let profile = self
            .store
            .agent_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if profile.owner != binding.owner {
            return Err(ProfileStoreError::Conflict);
        }
        if self.store.agent_profile_revision(&id, 1).await?.is_none() {
            if profile.draft != *content || profile.visibility != ProfileVisibility::Private {
                return Err(ProfileStoreError::Conflict);
            }
            self.store
                .publish_agent_profile(&id, profile.draft_version, binding.created_at.clone())
                .await?;
        }
        let revision = self
            .store
            .agent_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != *content
            || revision.digest != content_digest(content)?
        {
            return Err(ProfileStoreError::Conflict);
        }
        Ok(revision)
    }

    pub async fn workspace(
        &self,
        binding: &ProviderBinding,
    ) -> Result<WorkspaceProfileRevision, ProfileStoreError> {
        let content = &binding
            .profile
            .environment
            .as_ref()
            .ok_or(ProfileStoreError::NotFound)?
            .workspace;
        let id = WorkspaceProfileId(format!("wkp_{}", identity(binding, content)?));
        if self.store.workspace_profile(&id).await?.is_none() {
            created(
                self.store
                    .create_workspace_profile(WorkspaceProfile {
                        id: id.clone(),
                        owner: binding.owner.clone(),
                        name: binding.profile.name.clone(),
                        description: "Imported external binding profile".into(),
                        visibility: ProfileVisibility::Private,
                        forked_from: None,
                        uses_count: 0,
                        draft: content.clone(),
                        draft_version: 1,
                        created_at: binding.created_at.clone(),
                        updated_at: binding.created_at.clone(),
                        retired_at: None,
                    })
                    .await,
            )?;
        }
        let profile = self
            .store
            .workspace_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if profile.owner != binding.owner {
            return Err(ProfileStoreError::Conflict);
        }
        if self
            .store
            .workspace_profile_revision(&id, 1)
            .await?
            .is_none()
        {
            if profile.draft != *content || profile.visibility != ProfileVisibility::Private {
                return Err(ProfileStoreError::Conflict);
            }
            self.store
                .publish_workspace_profile(&id, profile.draft_version, binding.created_at.clone())
                .await?;
        }
        let revision = self
            .store
            .workspace_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != *content
            || revision.digest != content_digest(content)?
        {
            return Err(ProfileStoreError::Conflict);
        }
        Ok(revision)
    }

    pub async fn sandbox(
        &self,
        binding: &ProviderBinding,
    ) -> Result<SandboxProfileRevision, ProfileStoreError> {
        let content = &binding
            .profile
            .environment
            .as_ref()
            .ok_or(ProfileStoreError::NotFound)?
            .sandbox;
        let id = SandboxProfileId(format!("sbp_{}", identity(binding, content)?));
        if self.store.sandbox_profile(&id).await?.is_none() {
            created(
                self.store
                    .create_sandbox_profile(SandboxProfile {
                        availability: Default::default(),
                        id: id.clone(),
                        owner: binding.owner.clone(),
                        name: binding.profile.name.clone(),
                        description: "Imported external binding profile".into(),
                        visibility: ProfileVisibility::Private,
                        forked_from: None,
                        uses_count: 0,
                        draft: content.clone(),
                        draft_version: 1,
                        created_at: binding.created_at.clone(),
                        updated_at: binding.created_at.clone(),
                        retired_at: None,
                    })
                    .await,
            )?;
        }
        let profile = self
            .store
            .sandbox_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if profile.owner != binding.owner {
            return Err(ProfileStoreError::Conflict);
        }
        if self.store.sandbox_profile_revision(&id, 1).await?.is_none() {
            if profile.draft != *content || profile.visibility != ProfileVisibility::Private {
                return Err(ProfileStoreError::Conflict);
            }
            self.store
                .publish_sandbox_profile(
                    &id,
                    profile.draft_version,
                    binding.created_at.clone(),
                    self.bounds,
                    &PrivateImports,
                )
                .await?;
        }
        let revision = self
            .store
            .sandbox_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != *content
            || revision.digest != content_digest(content)?
        {
            return Err(ProfileStoreError::Conflict);
        }
        Ok(revision)
    }
}

fn identity(
    binding: &ProviderBinding,
    content: &impl serde::Serialize,
) -> Result<String, ProfileStoreError> {
    Ok(content_digest(&(
        &binding.binding_id,
        &binding.owner,
        content,
    ))?)
}

fn created(result: Result<(), ProfileStoreError>) -> Result<(), ProfileStoreError> {
    match result {
        Ok(()) | Err(ProfileStoreError::Duplicate) => Ok(()),
        Err(error) => Err(error),
    }
}

#[derive(Debug)]
struct PrivateImports;
#[async_trait::async_trait]
impl PublicValueScanner for PrivateImports {
    async fn scan(&self, _: &[EnvironmentVariable]) -> Result<(), PublicValueScanError> {
        Err(PublicValueScanError::Unavailable)
    }
}
