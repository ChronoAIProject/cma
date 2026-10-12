//! Private, immutable profile creation under freshly verified selected-owner authority.
use super::*;
use crate::{
    bots::{BotOwnerAuthority, BotProfileStore},
    Capability,
};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ProfileCreationError {
    #[error("profile creation is forbidden")]
    Forbidden,
    #[error("the creation key already names a different profile")]
    Conflict,
    #[error(transparent)]
    Invalid(#[from] ProfileError),
    #[error(transparent)]
    Store(#[from] ProfileStoreError),
}

pub struct PrivateSandboxProfileInput {
    pub name: String,
    pub description: String,
    pub content: SandboxProfileContent,
    pub availability: SandboxProfileAvailability,
}

/// Published revisions are immutable; their containing profile stays private.
/// A deterministic ID lets a durable setup resume between draft insertion and publication.
pub struct PrivateProfileCreationService {
    store: Arc<dyn BotProfileStore>,
    bounds: ProfileCapacityBounds,
    scanner: Arc<dyn PublicValueScanner>,
}
impl PrivateProfileCreationService {
    pub fn new(
        store: Arc<dyn BotProfileStore>,
        bounds: ProfileCapacityBounds,
        scanner: Arc<dyn PublicValueScanner>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            bounds,
            scanner,
        })
    }

    pub async fn create_agent(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        name: String,
        description: String,
        content: AgentProfileContent,
    ) -> Result<AgentProfileRevision, ProfileCreationError> {
        require(authority)?;
        let id = crate::AgentProfileId(profile_id(authority, "agp", key)?);
        let now = crate::now_rfc3339();
        let profile = AgentProfile {
            id: id.clone(),
            owner: authority.owner_subject().clone(),
            name,
            description,
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: content,
            draft_version: 1,
            created_at: now.clone(),
            updated_at: now,
            retired_at: None,
        };
        profile.validate()?;

        require(authority)?;
        match self.store.create_agent_profile(profile.clone()).await {
            Ok(()) | Err(ProfileStoreError::Duplicate) => {}
            Err(error) => return Err(error.into()),
        }
        let stored = self
            .store
            .agent_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if stored.owner != profile.owner
            || stored.name != profile.name
            || stored.description != profile.description
            || stored.visibility != ProfileVisibility::Private
            || stored.forked_from.is_some()
            || stored.draft != profile.draft
            || stored.draft_version != 1
            || stored.retired_at.is_some()
        {
            return Err(ProfileCreationError::Conflict);
        }
        if self.store.agent_profile_revision(&id, 1).await?.is_none() {
            require(authority)?;
            self.store
                .publish_agent_profile(&id, 1, crate::now_rfc3339())
                .await?;
        }
        let revision = self
            .store
            .agent_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        require(authority)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != profile.draft
            || revision.digest != content_digest(&profile.draft)?
        {
            return Err(ProfileCreationError::Conflict);
        }
        Ok(revision)
    }

    pub async fn create_workspace(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        name: String,
        description: String,
        content: WorkspaceProfileContent,
    ) -> Result<WorkspaceProfileRevision, ProfileCreationError> {
        require(authority)?;
        let id = crate::WorkspaceProfileId(profile_id(authority, "wkp", key)?);
        let now = crate::now_rfc3339();
        let profile = WorkspaceProfile {
            id: id.clone(),
            owner: authority.owner_subject().clone(),
            name,
            description,
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: content,
            draft_version: 1,
            created_at: now.clone(),
            updated_at: now,
            retired_at: None,
        };
        profile.validate()?;
        references::validate_agent_references(
            self.store.as_ref(),
            self.store.as_ref(),
            authority.owner_subject(),
            None,
            &profile.draft.agent_profiles,
        )
        .await
        .map_err(|e| e.error)?;
        require(authority)?;
        match self.store.create_workspace_profile(profile.clone()).await {
            Ok(()) | Err(ProfileStoreError::Duplicate) => {}
            Err(error) => return Err(error.into()),
        }
        let stored = self
            .store
            .workspace_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if stored.owner != profile.owner
            || stored.name != profile.name
            || stored.description != profile.description
            || stored.visibility != ProfileVisibility::Private
            || stored.forked_from.is_some()
            || stored.draft != profile.draft
            || stored.draft_version != 1
            || stored.retired_at.is_some()
        {
            return Err(ProfileCreationError::Conflict);
        }
        if self
            .store
            .workspace_profile_revision(&id, 1)
            .await?
            .is_none()
        {
            require(authority)?;
            self.store
                .publish_workspace_profile(&id, 1, crate::now_rfc3339())
                .await?;
        }
        let revision = self
            .store
            .workspace_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        require(authority)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != profile.draft
            || revision.digest != content_digest(&profile.draft)?
        {
            return Err(ProfileCreationError::Conflict);
        }
        Ok(revision)
    }

    pub async fn create_sandbox(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        name: String,
        description: String,
        content: SandboxProfileContent,
    ) -> Result<SandboxProfileRevision, ProfileCreationError> {
        self.create_sandbox_configured(
            authority,
            key,
            PrivateSandboxProfileInput {
                name,
                description,
                content,
                availability: SandboxProfileAvailability::default(),
            },
        )
        .await
    }

    pub async fn create_sandbox_configured(
        &self,
        authority: &BotOwnerAuthority,
        key: &str,
        input: PrivateSandboxProfileInput,
    ) -> Result<SandboxProfileRevision, ProfileCreationError> {
        require(authority)?;
        let PrivateSandboxProfileInput {
            name,
            description,
            content,
            availability,
        } = input;
        let id = crate::SandboxProfileId(profile_id(authority, "sbp", key)?);
        let now = crate::now_rfc3339();
        let profile = SandboxProfile {
            availability,
            id: id.clone(),
            owner: authority.owner_subject().clone(),
            name,
            description,
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: content,
            draft_version: 1,
            created_at: now.clone(),
            updated_at: now,
            retired_at: None,
        };
        profile.validate(self.bounds)?;
        references::validate_workspace_references(
            self.store.as_ref(),
            self.store.as_ref(),
            authority.owner_subject(),
            None,
            &profile.draft.workspace_profiles,
        )
        .await
        .map_err(|e| e.error)?;
        require(authority)?;
        match self.store.create_sandbox_profile(profile.clone()).await {
            Ok(()) | Err(ProfileStoreError::Duplicate) => {}
            Err(error) => return Err(error.into()),
        }
        let stored = self
            .store
            .sandbox_profile(&id)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        if stored.availability != profile.availability
            || stored.owner != profile.owner
            || stored.name != profile.name
            || stored.description != profile.description
            || stored.visibility != ProfileVisibility::Private
            || stored.forked_from.is_some()
            || stored.draft != profile.draft
            || stored.draft_version != 1
            || stored.retired_at.is_some()
        {
            return Err(ProfileCreationError::Conflict);
        }
        if self.store.sandbox_profile_revision(&id, 1).await?.is_none() {
            require(authority)?;
            self.store
                .publish_sandbox_profile(
                    &id,
                    1,
                    crate::now_rfc3339(),
                    self.bounds,
                    self.scanner.as_ref(),
                )
                .await?;
        }
        let revision = self
            .store
            .sandbox_profile_revision(&id, 1)
            .await?
            .ok_or(ProfileStoreError::NotFound)?;
        require(authority)?;
        if revision.profile_id != id
            || revision.revision != 1
            || revision.content != profile.draft
            || revision.digest != content_digest(&profile.draft)?
        {
            return Err(ProfileCreationError::Conflict);
        }
        Ok(revision)
    }
}
fn require(authority: &BotOwnerAuthority) -> Result<(), ProfileCreationError> {
    authority
        .require(Capability::ManageOwnProfiles)
        .map_err(|_| ProfileCreationError::Forbidden)
}
fn profile_id(
    authority: &BotOwnerAuthority,
    kind: &str,
    key: &str,
) -> Result<String, ProfileCreationError> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        return Err(ProfileError::new("invalid_creation_key", "key").into());
    }
    let digest = content_digest(&("private-owned-profile-v1", authority.owner(), kind, key))?;
    Ok(format!("{kind}_{}", &digest[..32]))
}
