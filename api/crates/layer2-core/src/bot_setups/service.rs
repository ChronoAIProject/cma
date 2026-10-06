use super::*;
use crate::bots::BotError;
use std::sync::Arc;
pub struct BotSetupService {
    pub(super) store: Arc<dyn BotSetupStore>,
    pub(super) dependencies: Arc<dyn BotSetupDependencies>,
}
impl BotSetupService {
    pub fn new(
        store: Arc<dyn BotSetupStore>,
        dependencies: Arc<dyn BotSetupDependencies>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            dependencies,
        })
    }

    pub async fn advance(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        draft: SetupDraft,
        customization: SetupCustomization,
    ) -> Result<SetupView, BotError> {
        self.advance_with_activity(binding, credential, reference, None, draft, customization)
            .await
    }

    pub async fn context(
        &self,
        binding: &str,
        credential: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<serde_json::Value>, BotError> {
        self.dependencies
            .context(binding, credential, conversation, event)
            .await
    }

    pub async fn advance_with_activity(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&serde_json::Value>,
        draft: SetupDraft,
        customization: SetupCustomization,
    ) -> Result<SetupView, BotError> {
        let event = self
            .dependencies
            .event(binding, credential, reference, activity)
            .await?;
        self.execute(event, draft, customization, None).await
    }

    pub async fn act(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        request: SetupActionRequest,
    ) -> Result<SetupView, BotError> {
        request.validate()?;
        let event = self
            .dependencies
            .event(binding, credential, reference, None)
            .await?;
        let (draft, customization) = request.content();
        self.execute(event, draft, customization, Some(&request))
            .await
    }

    pub(super) async fn choose_component(
        &self,
        setup: &mut BotSetup,
        action: &str,
    ) -> Result<(), BotError> {
        let (next, custom) = match setup.step {
            SetupStep::Agent => (SetupStep::Workspace, SetupStep::AgentCustom),
            SetupStep::Workspace => (SetupStep::Sandbox, SetupStep::WorkspaceCustom),
            SetupStep::Sandbox => (SetupStep::Services, SetupStep::SandboxCustom),
            _ => return Err(super::helpers::invalid_action()),
        };
        let step = super::helpers::choose(action, next, custom)?;
        if action == "defaults" {
            let defaults = self.dependencies.defaults().await?;
            match setup.step {
                SetupStep::Agent => {
                    setup.configuration.agent = defaults.agent;
                    if let Some(instructions) = &setup.draft.instructions {
                        setup.configuration.agent.system_prompt = instructions.clone();
                    }
                }
                SetupStep::Workspace => setup.configuration.workspace = defaults.workspace,
                SetupStep::Sandbox => {
                    setup.configuration.sandbox = defaults.sandbox;
                    setup.configuration.availability = defaults.availability;
                    setup.selected_services.clear();
                }
                _ => unreachable!("component step checked above"),
            }
            self.dependencies
                .validate_configuration(&setup.configuration)
                .await?;
        }
        setup.step = step;
        Ok(())
    }

    pub(super) async fn load(&self, actor: &ExternalActor) -> Result<BotSetup, BotError> {
        let setup = match self.store.get(actor).await? {
            Some(setup) => setup,
            None => {
                let setup = BotSetup {
                    id: format!("bst_{}", uuid::Uuid::new_v4().simple()),
                    actor: actor.clone(),
                    revision: 1,
                    step: SetupStep::Login,
                    grant_id: None,
                    owner: None,
                    draft: SetupDraft::default(),
                    configuration: self.dependencies.defaults().await?,
                    selected_services: vec![],
                    external_bot_id: None,
                    connection_ref: None,
                    pending_connection: None,
                    admission: None,
                    service_page: 0,
                    confirmation_digest: None,
                    confirmation_event: None,
                    processed_actions: vec![],
                    bot_id: None,
                    processed_events: vec![],
                    updated_at: crate::now_rfc3339(),
                };
                match self.store.insert(&setup).await {
                    Ok(()) => setup,
                    Err(BotError::Conflict) => {
                        self.store.get(actor).await?.ok_or(BotError::Conflict)?
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        Ok(setup)
    }
}
