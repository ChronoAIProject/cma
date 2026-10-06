use super::{helpers::grant, *};
use crate::{bots::BotError, resource_access::ResourceOwner};

impl BotSetupService {
    pub(super) async fn view(
        &self,
        setup: &BotSetup,
        mut notice: Option<String>,
        mut connection_url: Option<String>,
    ) -> Result<SetupView, BotError> {
        let identity = self.dependencies.identity(&setup.actor).await?;
        let mut choices = vec![];
        let mut services = vec![];
        let mut result = None;
        let mut step = setup.step;
        match setup.step {
            SetupStep::Login => {}
            SetupStep::Owner => {
                for entry in self
                    .dependencies
                    .owners(&setup.actor, grant(setup)?)
                    .await?
                {
                    let id = match entry.owner {
                        ResourceOwner::Person { .. } => "personal".into(),
                        ResourceOwner::Organization { subject } => {
                            format!("org:{}", subject.as_str())
                        }
                    };
                    choices.push(SetupChoice::new(id, entry.name));
                }
            }
            SetupStep::Agent | SetupStep::Workspace | SetupStep::Sandbox => {
                choices.push(SetupChoice::new("defaults", "Use system defaults"));
                choices.push(SetupChoice::new("custom", "Customize"));
            }
            SetupStep::AgentCustom => {
                choices.push(SetupChoice::new(
                    "agent:quick",
                    "Quick responses (low reasoning)",
                ));
                choices.push(SetupChoice::new(
                    "agent:thorough",
                    "Thorough answers (high reasoning)",
                ));
                choices.push(SetupChoice::new("done", "Done editing"));
                choices.push(SetupChoice::new("back", "Back"));
            }
            SetupStep::WorkspaceCustom => {
                choices.push(SetupChoice::new(
                    "workspace:prepared",
                    "Keep the system setup commands",
                ));
                choices.push(SetupChoice::new(
                    "workspace:clean",
                    "Start with a clean workspace",
                ));
                choices.push(SetupChoice::new("done", "Done editing"));
                choices.push(SetupChoice::new("back", "Back"));
            }
            SetupStep::SandboxCustom => {
                choices.push(SetupChoice::new("sandbox:on_demand", "Start on demand"));
                choices.push(SetupChoice::new(
                    "sandbox:always_live",
                    "Keep the sandbox always live",
                ));
                choices.push(SetupChoice::new("done", "Done editing"));
                choices.push(SetupChoice::new("back", "Back"));
            }
            SetupStep::Services => {
                services = self.catalog(setup).await?;
                for service in services.iter().skip(setup.service_page * 16).take(16) {
                    choices.push(SetupChoice {
                        id: format!("service:{}", service.id),
                        label: service.name.clone(),
                        selected: service.selected,
                    });
                }
                if setup.service_page > 0 {
                    choices.push(SetupChoice::new("previous", "Previous services"));
                }
                if services.len() > (setup.service_page + 1) * 16 {
                    choices.push(SetupChoice::new("next", "More services"));
                }
                choices.push(SetupChoice::new("done", "Continue"));
            }
            SetupStep::ExternalBot => {
                for bot in self
                    .dependencies
                    .external_bots(&setup.actor, grant(setup)?, super::helpers::owner(setup)?)
                    .await?
                    .into_iter()
                    .take(16)
                {
                    choices.push(SetupChoice::new(format!("bot:{}", bot.id), bot.name));
                }
                choices.push(SetupChoice::new("register", "Create or connect a channel"));
                choices.push(SetupChoice::new("refresh", "Refresh channels"));
            }
            SetupStep::Admission => {
                choices.push(SetupChoice::new("private", "Only me"));
                choices.push(SetupChoice::new("public", "Anyone in a private chat"));
                choices.push(SetupChoice::new(
                    "groups",
                    "Private chats and group mentions",
                ));
            }
            SetupStep::Review => {
                services = self.catalog(setup).await?;
                choices.push(SetupChoice::new(
                    format!(
                        "create:{}",
                        setup
                            .confirmation_digest
                            .as_deref()
                            .ok_or(BotError::Conflict)?
                    ),
                    "Create Bot",
                ));
                choices.push(SetupChoice::new("edit", "Change configuration"));
            }
            SetupStep::Provisioning | SetupStep::Complete => {
                if setup.bot_id.is_some() {
                    let current = self.dependencies.result(setup).await?;
                    if current.state == "active" {
                        step = SetupStep::Complete;
                    }
                    result = Some(current);
                }
                choices.push(SetupChoice::new("refresh", "Check progress"));
                if step == SetupStep::Complete {
                    choices.push(SetupChoice::new("new", "Create another Bot"));
                }
            }
            SetupStep::Cancelled => choices.push(SetupChoice::new("new", "Create another Bot")),
            SetupStep::Name => {}
        }
        if matches!(
            setup.step,
            SetupStep::Name
                | SetupStep::Agent
                | SetupStep::AgentCustom
                | SetupStep::Workspace
                | SetupStep::WorkspaceCustom
                | SetupStep::Sandbox
                | SetupStep::SandboxCustom
                | SetupStep::Services
                | SetupStep::ExternalBot
                | SetupStep::Admission
        ) {
            choices.push(SetupChoice::new("edit", "Change configuration"));
        }
        if !matches!(
            setup.step,
            SetupStep::Provisioning | SetupStep::Complete | SetupStep::Cancelled
        ) {
            choices.push(SetupChoice::new("cancel", "Cancel"));
        }
        if let Some(pending) = &setup.pending_connection {
            connection_url = pending.setup_url.clone();
            if pending.terminal() && notice.is_none() {
                notice = Some("The channel connection did not complete. Choose Create or connect a channel to try again.".into());
            }
        }
        if setup.step == SetupStep::Login {
            connection_url = identity.login_url.clone();
        }
        Ok(SetupView {
            setup_id: setup.id.clone(),
            revision: setup.revision,
            step,
            interaction_id: setup.block_id(),
            identity,
            owner: setup.owner.clone(),
            name: setup.draft.name.clone(),
            configuration: setup.configuration.clone(),
            choices,
            services,
            connection_url,
            external_bot_id: setup.external_bot_id.clone(),
            connection_ref: setup.connection_ref.clone(),
            connection_status: setup
                .pending_connection
                .as_ref()
                .map(|pending| pending.status.clone())
                .or_else(|| setup.connection_ref.as_ref().map(|_| "ready".into())),
            admission: setup.admission.clone(),
            confirmation_digest: setup.confirmation_digest.clone(),
            result,
            notice,
        })
    }
}
