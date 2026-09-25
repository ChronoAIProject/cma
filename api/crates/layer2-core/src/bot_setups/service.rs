use super::*;
use crate::{
    bots::{BotAdmission, BotError, GroupAdmission, IdentityAdmission},
    resource_access::ResourceOwner,
    SubjectId,
};
use std::sync::Arc;

use super::helpers::*;

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
        let event = self
            .dependencies
            .event(binding, credential, reference)
            .await?;
        let mut setup = match self.store.get(&event.actor).await? {
            Some(setup) => setup,
            None => {
                let setup = BotSetup {
                    id: format!("bst_{}", uuid::Uuid::new_v4().simple()),
                    actor: event.actor.clone(),
                    revision: 1,
                    step: SetupStep::Login,
                    grant_id: None,
                    owner: None,
                    draft: SetupDraft::default(),
                    configuration: self.dependencies.defaults().await?,
                    selected_services: vec![],
                    external_bot_id: None,
                    admission: None,
                    service_page: 0,
                    confirmation_digest: None,
                    bot_id: None,
                    processed_events: vec![],
                    updated_at: crate::now_rfc3339(),
                };
                match self.store.insert(&setup).await {
                    Ok(()) => setup,
                    Err(BotError::Conflict) => self
                        .store
                        .get(&event.actor)
                        .await?
                        .ok_or(BotError::Conflict)?,
                    Err(error) => return Err(error),
                }
            }
        };
        if setup.actor != event.actor {
            return Err(BotError::Forbidden);
        }
        if setup.processed_events.contains(&event.event_id) {
            return self.finish_or_view(setup, None).await;
        }
        let previous_revision = setup.revision;
        let action = match &event.input {
            SetupInput::Interaction {
                block_id,
                action_id,
            } => {
                if block_id != &setup.block_id() {
                    return self
                        .view(
                            &setup,
                            Some("This choice has expired. Use the latest configuration.".into()),
                            None,
                        )
                        .await;
                }
                Some(action_id.as_str())
            }
            SetupInput::Text(_) => None,
        };
        let text = match &event.input {
            SetupInput::Text(text) => Some(text.as_str()),
            _ => None,
        };
        let mut connection_url = None;
        let mut notice = None;
        if action == Some("cancel")
            && !matches!(setup.step, SetupStep::Provisioning | SetupStep::Complete)
        {
            setup.step = SetupStep::Cancelled;
        } else if matches!(setup.step, SetupStep::Complete | SetupStep::Cancelled)
            && action == Some("new")
        {
            setup.id = format!("bst_{}", uuid::Uuid::new_v4().simple());
            setup.step = SetupStep::Login;
            setup.grant_id = None;
            setup.owner = None;
            setup.draft = SetupDraft::default();
            setup.configuration = self.dependencies.defaults().await?;
            setup.selected_services.clear();
            setup.external_bot_id = None;
            setup.admission = None;
            setup.confirmation_digest = None;
            setup.bot_id = None;
            setup.service_page = 0;
        } else {
            let identity = if setup.step == SetupStep::Login {
                match text.and_then(login_proof) {
                    Some(proof) => {
                        self.dependencies
                            .confirm_identity(&setup.actor, proof)
                            .await?
                    }
                    None => self.dependencies.identity(&setup.actor).await?,
                }
            } else {
                self.dependencies.identity(&setup.actor).await?
            };
            if !matches!(setup.step, SetupStep::Complete | SetupStep::Cancelled) {
                if identity.grant_id.is_none()
                    || (setup.grant_id.is_some() && setup.grant_id != identity.grant_id)
                {
                    setup.step = SetupStep::Login;
                    setup.grant_id = None;
                    setup.owner = None;
                    setup.confirmation_digest = None;
                } else if setup.step == SetupStep::Login {
                    setup.grant_id = identity.grant_id;
                    setup.step = SetupStep::Owner;
                }
            }
            if text.is_some()
                && matches!(
                    setup.step,
                    SetupStep::Login | SetupStep::Owner | SetupStep::Name
                )
            {
                apply_draft(&mut setup, &draft)?;
            }
            // A fresh login changes the required step. Never interpret that same
            // input as an owner choice, creation consent or unrelated mutation.
            if let Some(action) = action {
                match setup.step {
                    SetupStep::Owner => {
                        let owners = self
                            .dependencies
                            .owners(&setup.actor, grant(&setup)?)
                            .await?;
                        let owner = if action == "personal" {
                            let subject = identity.subject.ok_or(BotError::Forbidden)?;
                            ResourceOwner::Person {
                                subject: SubjectId::new(subject),
                            }
                        } else if let Some(id) = action.strip_prefix("org:") {
                            ResourceOwner::Organization {
                                subject: SubjectId::new(id),
                            }
                        } else {
                            return Err(invalid_action());
                        };
                        if !owners.iter().any(|candidate| candidate.owner == owner) {
                            return Err(BotError::Forbidden);
                        }
                        setup.owner = Some(owner);
                        setup.step = if setup.draft.name.is_some() {
                            SetupStep::Agent
                        } else {
                            SetupStep::Name
                        };
                    }
                    SetupStep::Agent => {
                        setup.step = choose(action, SetupStep::Workspace, SetupStep::AgentCustom)?
                    }
                    SetupStep::Workspace => {
                        setup.step = choose(action, SetupStep::Sandbox, SetupStep::WorkspaceCustom)?
                    }
                    SetupStep::Sandbox => {
                        setup.step = choose(action, SetupStep::Services, SetupStep::SandboxCustom)?
                    }
                    SetupStep::AgentCustom
                    | SetupStep::WorkspaceCustom
                    | SetupStep::SandboxCustom
                        if action == "back" =>
                    {
                        setup.step = match setup.step {
                            SetupStep::AgentCustom => SetupStep::Agent,
                            SetupStep::WorkspaceCustom => SetupStep::Workspace,
                            _ => SetupStep::Sandbox,
                        };
                    }
                    SetupStep::Services => {
                        let services = self.catalog(&setup).await?;
                        if action == "done" {
                            let missing = services.iter().find(|s| s.selected && !s.connected);
                            if let Some(service) = missing {
                                connection_url = Some(
                                    self.dependencies
                                        .connect(
                                            &setup.actor,
                                            grant(&setup)?,
                                            owner(&setup)?,
                                            &service.id,
                                        )
                                        .await?,
                                );
                                notice = Some(format!(
                                    "Connect {} for the selected owner before continuing.",
                                    service.name
                                ));
                            } else {
                                setup.selected_services = services
                                    .iter()
                                    .filter(|s| s.selected)
                                    .map(|s| s.id.clone())
                                    .collect();
                                setup.configuration.sandbox.nyxid_services = services
                                    .iter()
                                    .filter(|s| s.selected)
                                    .map(|s| crate::ServiceId::new(&s.id))
                                    .collect();
                                setup.step = SetupStep::ExternalBot;
                            }
                        } else if action == "next" {
                            setup.service_page = setup
                                .service_page
                                .saturating_add(1)
                                .min(services.len().saturating_sub(1) / 16);
                        } else if action == "previous" {
                            setup.service_page = setup.service_page.saturating_sub(1);
                        } else if let Some(id) = action.strip_prefix("service:") {
                            let service = services
                                .iter()
                                .find(|s| s.id == id)
                                .ok_or(BotError::Forbidden)?;
                            setup.selected_services = services
                                .iter()
                                .filter(|s| s.selected)
                                .map(|s| s.id.clone())
                                .collect();
                            if service.selected && !service.required {
                                setup.selected_services.retain(|s| s != id);
                            } else if !service.selected {
                                setup.selected_services.push(id.into());
                            }
                            if (!service.selected || service.required) && !service.connected {
                                connection_url = Some(
                                    self.dependencies
                                        .connect(&setup.actor, grant(&setup)?, owner(&setup)?, id)
                                        .await?,
                                );
                            }
                            // Subsequent catalog reads use these explicit IDs,
                            // not the old default profile's service selections.
                            setup.configuration.sandbox.nyxid_services = services
                                .iter()
                                .filter(|s| setup.selected_services.contains(&s.id))
                                .map(|s| crate::ServiceId::new(&s.slug))
                                .collect();
                        } else {
                            return Err(invalid_action());
                        }
                    }
                    SetupStep::ExternalBot => {
                        if action == "register" {
                            connection_url = Some(
                                self.dependencies
                                    .external_bot_registration_url(owner(&setup)?)
                                    .await?,
                            );
                            notice = Some(
                                "Register a bot you control, then return and refresh the list."
                                    .into(),
                            );
                        } else if action != "refresh" {
                            let id = action.strip_prefix("bot:").ok_or_else(invalid_action)?;
                            let bots = self
                                .dependencies
                                .external_bots(&setup.actor, grant(&setup)?, owner(&setup)?)
                                .await?;
                            if !bots.iter().any(|bot| bot.id == id) {
                                return Err(BotError::Forbidden);
                            }
                            setup.external_bot_id = Some(id.into());
                            setup.step = SetupStep::Admission;
                        }
                    }
                    SetupStep::Admission => {
                        setup.admission = Some(match action {
                            "private" => BotAdmission::Scoped {
                                senders: IdentityAdmission::Allowlist(vec![setup
                                    .actor
                                    .actor_id
                                    .clone()]),
                                chats: IdentityAdmission::Open,
                                groups: GroupAdmission::Deny,
                            },
                            "public" => BotAdmission::Scoped {
                                senders: IdentityAdmission::Open,
                                chats: IdentityAdmission::Open,
                                groups: GroupAdmission::Deny,
                            },
                            "groups" => BotAdmission::Scoped {
                                senders: IdentityAdmission::Open,
                                chats: IdentityAdmission::Open,
                                groups: GroupAdmission::MentionOrReplyToBot,
                            },
                            _ => return Err(invalid_action()),
                        });
                        self.dependencies
                            .validate_configuration(&setup.configuration)
                            .await?;
                        setup.step = SetupStep::Review;
                        setup.confirmation_digest = Some(setup.plan_digest()?);
                    }
                    SetupStep::Review => {
                        if action == "edit" {
                            setup.step = SetupStep::Owner;
                            setup.confirmation_digest = None;
                        } else if action
                            == format!(
                                "create:{}",
                                setup
                                    .confirmation_digest
                                    .as_deref()
                                    .ok_or(BotError::Conflict)?
                            )
                        {
                            if setup.confirmation_digest.as_deref()
                                != Some(setup.plan_digest()?.as_str())
                            {
                                return Err(BotError::Conflict);
                            }
                            setup.step = SetupStep::Provisioning;
                        } else {
                            return Err(invalid_action());
                        }
                    }
                    SetupStep::Login
                    | SetupStep::Provisioning
                    | SetupStep::Complete
                    | SetupStep::Cancelled => {}
                    _ => return Err(invalid_action()),
                }
            } else if text.is_some() {
                match setup.step {
                    SetupStep::Name if setup.draft.name.is_some() => setup.step = SetupStep::Agent,
                    SetupStep::AgentCustom => {
                        setup.configuration.agent = customization.agent.ok_or_else(|| {
                            BotError::Invalid(
                                "Provide the customized Agent Profile configuration.".into(),
                            )
                        })?;
                        self.dependencies
                            .validate_configuration(&setup.configuration)
                            .await?;
                        setup.step = SetupStep::Workspace;
                    }
                    SetupStep::WorkspaceCustom => {
                        setup.configuration.workspace =
                            customization.workspace.ok_or_else(|| {
                                BotError::Invalid(
                                    "Provide the customized Workspace Profile configuration."
                                        .into(),
                                )
                            })?;
                        self.dependencies
                            .validate_configuration(&setup.configuration)
                            .await?;
                        setup.step = SetupStep::Sandbox;
                    }
                    SetupStep::SandboxCustom => {
                        setup.configuration.sandbox = customization.sandbox.ok_or_else(|| {
                            BotError::Invalid(
                                "Provide the customized Sandbox Profile configuration.".into(),
                            )
                        })?;
                        if let Some(availability) = customization.availability {
                            setup.configuration.availability = availability;
                        }
                        self.dependencies
                            .validate_configuration(&setup.configuration)
                            .await?;
                        setup.selected_services.clear();
                        setup.step = SetupStep::Services;
                    }
                    _ => {}
                }
            }
        }
        setup.revision = setup.revision.checked_add(1).ok_or(BotError::Conflict)?;
        setup.updated_at = crate::now_rfc3339();
        setup.processed_events.push(event.event_id);
        if setup.processed_events.len() > 128 {
            setup.processed_events.remove(0);
        }
        self.store.replace(&setup, previous_revision).await?;
        if setup.step == SetupStep::Provisioning {
            return self.finish_or_view(setup, notice).await;
        }
        self.view(&setup, notice, connection_url).await
    }

    async fn finish_or_view(
        &self,
        mut setup: BotSetup,
        notice: Option<String>,
    ) -> Result<SetupView, BotError> {
        if setup.step == SetupStep::Provisioning && setup.bot_id.is_none() {
            let result = self.dependencies.provision(&setup).await?;
            let previous = setup.revision;
            setup.bot_id = Some(result.bot_id);
            setup.revision += 1;
            setup.updated_at = crate::now_rfc3339();
            self.store.replace(&setup, previous).await?;
        }
        if setup.step == SetupStep::Provisioning && setup.bot_id.is_some() {
            let result = self.dependencies.result(&setup).await?;
            if result.state == "active" {
                let previous = setup.revision;
                setup.step = SetupStep::Complete;
                setup.revision += 1;
                setup.updated_at = crate::now_rfc3339();
                self.store.replace(&setup, previous).await?;
            }
        }
        self.view(&setup, notice, None).await
    }

    pub(super) async fn catalog(
        &self,
        setup: &BotSetup,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        let mut services = self
            .dependencies
            .services(&setup.actor, grant(setup)?, owner(setup)?)
            .await?;
        for service in &mut services {
            service.selected = service.required
                || setup.selected_services.contains(&service.id)
                || setup
                    .configuration
                    .sandbox
                    .nyxid_services
                    .iter()
                    .any(|id| id.as_str() == service.id || id.as_str() == service.slug);
        }
        services.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(services)
    }
}
