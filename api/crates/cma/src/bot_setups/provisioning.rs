use super::*;
use layer2_core::{
    bots::{Bot, BotReply, BotState, CreateBot},
    profiles::{
        AgentProfileRef, PrivateSandboxProfileInput, ProfileCreationError, SandboxProfileRef,
        WorkspaceProfileRef,
    },
    sandbox::binding::BindingStore,
};

impl SetupDependencies {
    pub(super) async fn create(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        if setup.step != SetupStep::Provisioning
            || setup.confirmation_digest.as_deref() != Some(setup.plan_digest()?.as_str())
        {
            return Err(BotError::Conflict);
        }
        self.validate(&setup.configuration).await?;
        let grant = setup.grant_id.as_deref().ok_or(BotError::Forbidden)?;
        let owner = setup.owner.as_ref().ok_or(BotError::Forbidden)?;
        let actor = authorization::actor(&setup.actor);
        let authority = self
            .auth
            .owner_authority_for_grant(grant, &actor, owner)
            .await?;
        let services = self.service_choices(&setup.actor, grant, owner).await?;
        for required in &self.required_services {
            if !services.iter().any(|s| {
                &s.slug == required && s.connected && setup.selected_services.contains(&s.id)
            }) {
                return Err(BotError::Invalid(format!(
                    "Connect and select the required service {required} for this owner."
                )));
            }
        }
        for selected in &setup.selected_services {
            if !services.iter().any(|s| &s.id == selected && s.connected) {
                return Err(BotError::Invalid(
                    "A selected service is no longer connected for this owner.".into(),
                ));
            }
        }
        if setup
            .configuration
            .sandbox
            .nyxid_services
            .iter()
            .map(|s| s.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            != setup.selected_services.iter().map(|s| s.as_str()).collect()
        {
            return Err(BotError::Conflict);
        }
        if let Some(reference) = &setup.connection_ref {
            let (_, token) = self.auth.context_for_grant(grant, &actor, owner).await?;
            if self
                .connections
                .get(token.expose(), owner, reference)
                .await?
                .status
                != "ready"
            {
                return Err(BotError::Conflict);
            }
        } else {
            // Retained pre-connection setup plans cannot silently gain a different source.
            return Err(BotError::Invalid(
                "Select a channel connection before creating the Bot.".into(),
            ));
        }
        let name = setup.draft.name.as_deref().ok_or(BotError::Conflict)?;
        let description = "Created through the owner-authorized Bot setup workflow";
        let agent = self
            .profiles
            .create_agent(
                &authority,
                &setup.id,
                name.into(),
                description.into(),
                setup.configuration.agent.clone(),
            )
            .await
            .map_err(profile_error)?;
        let agent_ref = AgentProfileRef {
            id: agent.profile_id,
            revision: agent.revision,
        };
        let mut workspace = setup.configuration.workspace.clone();
        workspace.agent_profiles = vec![agent_ref.clone()];
        let authority = self
            .auth
            .owner_authority_for_grant(grant, &actor, owner)
            .await?;
        let workspace = self
            .profiles
            .create_workspace(
                &authority,
                &setup.id,
                name.into(),
                description.into(),
                workspace,
            )
            .await
            .map_err(profile_error)?;
        let workspace_ref = WorkspaceProfileRef {
            id: workspace.profile_id,
            revision: workspace.revision,
        };
        let mut sandbox = setup.configuration.sandbox.clone();
        sandbox.workspace_profiles = vec![workspace_ref.clone()];
        let authority = self
            .auth
            .owner_authority_for_grant(grant, &actor, owner)
            .await?;
        let sandbox = self
            .profiles
            .create_sandbox_configured(
                &authority,
                &setup.id,
                PrivateSandboxProfileInput {
                    name: name.into(),
                    description: description.into(),
                    content: sandbox,
                    availability: setup.configuration.availability,
                },
            )
            .await
            .map_err(profile_error)?;
        let authority = self
            .auth
            .owner_authority_for_grant(grant, &actor, owner)
            .await?;
        let bot = self
            .bots
            .create_for_owner(
                &authority,
                &setup.id,
                CreateBot {
                    event_worker_profile: None,
                    name: name.into(),
                    external_bot_id: String::new(),
                    connection_ref: setup.connection_ref.clone(),
                    profile: agent_ref,
                    sandbox_profile: Some(SandboxProfileRef {
                        id: sandbox.profile_id,
                        revision: sandbox.revision,
                    }),
                    workspace_profile: Some(workspace_ref),
                    admission: setup.admission.clone().ok_or(BotError::Conflict)?,
                    reply: BotReply {
                        auto_final: true,
                        ack_after_ms: 1500,
                    },
                },
            )
            .await?;
        self.result_for_bot(setup, &bot).await
    }

    pub(super) async fn current_result(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        let authority = self
            .auth
            .owner_authority_for_grant(
                setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
                &authorization::actor(&setup.actor),
                setup.owner.as_ref().ok_or(BotError::Forbidden)?,
            )
            .await?;
        let bot = self
            .bots
            .get_for_owner(
                &authority,
                setup.bot_id.as_deref().ok_or(BotError::NotFound)?,
            )
            .await?;
        self.result_for_bot(setup, &bot).await
    }

    async fn result_for_bot(&self, setup: &BotSetup, bot: &Bot) -> Result<SetupResult, BotError> {
        let (state, failure) = match &bot.state {
            BotState::Creating => ("creating", None),
            BotState::Active => ("active", None),
            BotState::Disabled => ("disabled", None),
            BotState::Failed { reason } => ("failed", Some(reason.clone())),
            BotState::Deleting => ("deleting", None),
            BotState::Deleted => ("deleted", None),
        };
        let mut result = SetupResult {
            bot_id: bot.bot_id.clone(),
            state: state.into(),
            failure,
            operation: bot.work.as_ref().map(|w| w.step.clone()),
            sandbox_id: None,
            workspace_id: None,
            agent_id: None,
            bot_url: None,
        };
        if let Some(channel) = &bot.resources.channel {
            if let Some(binding) = self
                .stores
                .sandbox_model
                .binding(&channel.binding_id)
                .await
                .map_err(|_| BotError::Unavailable)?
            {
                if binding.owner != bot.owner {
                    return Err(BotError::Forbidden);
                }
                result.sandbox_id = Some(binding.sandbox_id.to_string());
                result.workspace_id = Some(binding.workspace_id.to_string());
                let conversations = self
                    .stores
                    .sandbox_model
                    .binding_conversations(&binding.binding_id, binding.generation, None, 2)
                    .await
                    .map_err(|_| BotError::Unavailable)?;
                if conversations.len() == 1 {
                    result.agent_id = Some(conversations[0].agent_id.to_string());
                }
            }
        }
        if let Some(reference) = &bot.request.connection_ref {
            let (_, token) = self
                .auth
                .context_for_grant(
                    setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
                    &authorization::actor(&setup.actor),
                    setup.owner.as_ref().ok_or(BotError::Forbidden)?,
                )
                .await?;
            result.bot_url = self
                .connections
                .get(token.expose(), &bot.resource_owner(), reference)
                .await?
                .url;
        }
        Ok(result)
    }
}
fn profile_error(error: ProfileCreationError) -> BotError {
    match error {
        ProfileCreationError::Forbidden => BotError::Forbidden,
        ProfileCreationError::Conflict => BotError::Conflict,
        ProfileCreationError::Invalid(error) => BotError::Invalid(error.to_string()),
        ProfileCreationError::Store(_) => BotError::Unavailable,
    }
}
