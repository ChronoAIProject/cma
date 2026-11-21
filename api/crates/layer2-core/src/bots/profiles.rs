//! Pin published defaults or personally owned environments alongside the Agent revision.
use super::{BotError, BotProfileResolver, BotProfiles, CreateBot};
use crate::{
    profiles::{
        content_digest,
        references::{reference_is_readable, verify_agent_revision, verify_workspace_revision},
        store::{AgentProfileStore, SandboxProfileStore, WorkspaceProfileStore},
        ProfileStoreError,
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
        request: &CreateBot,
    ) -> Result<BotProfiles, BotError> {
        let defaults = self
            .settings
            .current()
            .await
            .map_err(|_| BotError::Unavailable)?
            .settings
            .defaults;
        let reference = &request.profile;
        let sandbox_ref = request
            .sandbox_profile
            .as_ref()
            .unwrap_or(&defaults.sandbox_profile);
        let workspace_ref = request
            .workspace_profile
            .as_ref()
            .unwrap_or(&defaults.workspace_profile);
        let agent = self
            .store
            .agent_profile(&reference.id)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let sandbox = self
            .store
            .sandbox_profile(&sandbox_ref.id)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let workspace = self
            .store
            .workspace_profile(&workspace_ref.id)
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
        // Defaults are operator-selected. Explicit environments must belong to
        // this owner; foreign install commands need a separate trust contract.
        if (request.sandbox_profile.is_some() && &sandbox.owner != owner)
            || (request.workspace_profile.is_some() && &workspace.owner != owner)
        {
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
            .sandbox_profile_revision(&sandbox_ref.id, sandbox_ref.revision)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        let workspace = self
            .store
            .workspace_profile_revision(&workspace_ref.id, workspace_ref.revision)
            .await
            .map_err(error)?
            .ok_or(BotError::NotFound)?;
        verify_agent_revision(reference, &agent).map_err(error)?;
        verify_workspace_revision(workspace_ref, &workspace).map_err(error)?;
        if sandbox.profile_id != sandbox_ref.id
            || sandbox.revision != sandbox_ref.revision
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
            event_worker_profile: None,
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
