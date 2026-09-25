use super::*;
use layer2_core::profiles::{
    store::{AgentProfileStore, SandboxProfileStore, WorkspaceProfileStore},
    ModelAccess, ProfileError, ProfileVisibility,
};

impl SetupDependencies {
    pub(super) async fn default_configuration(&self) -> Result<SetupConfiguration, BotError> {
        let defaults = self
            .settings
            .current()
            .await
            .map_err(|_| BotError::Unavailable)?
            .settings
            .defaults;
        let store = &self.stores.sandbox_profiles;
        let agent = store
            .agent_profile_revision(&defaults.agent_profile.id, defaults.agent_profile.revision)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Unavailable)?;
        let workspace = store
            .workspace_profile_revision(
                &defaults.workspace_profile.id,
                defaults.workspace_profile.revision,
            )
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Unavailable)?;
        let sandbox = store
            .sandbox_profile_revision(
                &defaults.sandbox_profile.id,
                defaults.sandbox_profile.revision,
            )
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Unavailable)?;
        let mut configuration = SetupConfiguration {
            agent: agent.content,
            workspace: workspace.content,
            sandbox: sandbox.content,
            availability: Default::default(),
        };
        // This workflow creates one linked hierarchy. No reference from an
        // unrelated default hierarchy silently creates additional children.
        configuration.workspace.agent_profiles.clear();
        configuration.sandbox.workspace_profiles.clear();
        self.validate(&configuration).await?;
        Ok(configuration)
    }
    pub(super) async fn validate(
        &self,
        configuration: &SetupConfiguration,
    ) -> Result<(), BotError> {
        configuration.agent.validate().map_err(invalid)?;
        configuration.workspace.validate().map_err(invalid)?;
        configuration
            .sandbox
            .validate(crate::profile_policy::profile_route_policy().capacity_bounds)
            .map_err(invalid)?;
        configuration
            .availability
            .validate(ProfileVisibility::Private)
            .map_err(invalid)?;
        if configuration.sandbox.model_access != ModelAccess::ChronoaiManaged {
            return Err(BotError::Invalid(
                "Bot setup currently requires managed model access.".into(),
            ));
        }
        if !configuration.workspace.agent_profiles.is_empty()
            || !configuration.sandbox.workspace_profiles.is_empty()
        {
            return Err(BotError::Invalid("Configure the Agent, Workspace and Sandbox in this setup; embedded references to other profiles are not accepted.".into()));
        }
        Ok(())
    }
}
fn invalid(error: ProfileError) -> BotError {
    BotError::Invalid(error.to_string())
}
