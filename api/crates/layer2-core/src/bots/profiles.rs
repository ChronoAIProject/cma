//! Pin readable, published environment defaults alongside the selected Agent revision.
use super::{BotError, BotProfileResolver, BotProfiles};
use crate::{
    profiles::{
        content_digest,
        references::{reference_is_readable, verify_agent_revision, verify_workspace_revision},
        store::{AgentProfileStore, SandboxProfileStore, WorkspaceProfileStore},
        AgentProfileRef, ProfileStoreError,
    },
    PlatformSettingsService, SubjectId,
};
use std::sync::Arc;
pub trait BotProfileStore: AgentProfileStore + WorkspaceProfileStore + SandboxProfileStore {}
impl<T: AgentProfileStore + WorkspaceProfileStore + SandboxProfileStore> BotProfileStore for T {}
pub struct PublishedBotProfiles {
    store: Arc<dyn BotProfileStore>,
    settings: Arc<PlatformSettingsService>,
    default_model: String,
}
impl PublishedBotProfiles {
    pub fn new(
        store: Arc<dyn BotProfileStore>,
        settings: Arc<PlatformSettingsService>,
        default_model: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            settings,
            default_model,
        })
    }
}
#[async_trait::async_trait]
impl BotProfileResolver for PublishedBotProfiles {
    async fn resolve(
        &self,
        owner: &SubjectId,
        reference: &AgentProfileRef,
    ) -> Result<BotProfiles, BotError> {
        let defaults = self
            .settings
            .current()
            .await
            .map_err(|_| BotError::Unavailable)?
            .settings
            .defaults;
        let agent = self
            .store
            .agent_profile(&reference.id)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let sandbox = self
            .store
            .sandbox_profile(&defaults.sandbox_profile.id)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let workspace = self
            .store
            .workspace_profile(&defaults.workspace_profile.id)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        if !reference_is_readable(
            owner,
            &agent.owner,
            agent.visibility,
            agent.retired_at.is_some(),
            false,
        ) || !reference_is_readable(
            owner,
            &sandbox.owner,
            sandbox.visibility,
            sandbox.retired_at.is_some(),
            false,
        ) || !reference_is_readable(
            owner,
            &workspace.owner,
            workspace.visibility,
            workspace.retired_at.is_some(),
            false,
        ) {
            return Err(BotError::NotFound);
        }
        let agent = self
            .store
            .agent_profile_revision(&reference.id, reference.revision)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let sandbox = self
            .store
            .sandbox_profile_revision(
                &defaults.sandbox_profile.id,
                defaults.sandbox_profile.revision,
            )
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let workspace = self
            .store
            .workspace_profile_revision(
                &defaults.workspace_profile.id,
                defaults.workspace_profile.revision,
            )
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        verify_agent_revision(reference, &agent).map_err(error)?;
        verify_workspace_revision(&defaults.workspace_profile, &workspace).map_err(error)?;
        if sandbox.profile_id != defaults.sandbox_profile.id
            || sandbox.revision != defaults.sandbox_profile.revision
            || content_digest(&sandbox.content).map_err(|_| BotError::Unavailable)?
                != sandbox.digest
        {
            return Err(BotError::Unavailable);
        }
        let resolved_model = agent
            .content
            .model
            .clone()
            .unwrap_or_else(|| self.default_model.clone());
        if resolved_model.trim().is_empty() {
            return Err(BotError::Invalid(
                "no model is configured for this profile".into(),
            ));
        }
        Ok(BotProfiles {
            agent,
            sandbox,
            workspace,
            resolved_model,
        })
    }
}
fn error(error: ProfileStoreError) -> BotError {
    match error {
        ProfileStoreError::NotFound | ProfileStoreError::ReferenceUnavailable => BotError::NotFound,
        _ => BotError::Unavailable,
    }
}
