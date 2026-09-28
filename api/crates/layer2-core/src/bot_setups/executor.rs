use super::helpers::*;
use super::*;
use crate::{
    bots::{BotAdmission, BotError, GroupAdmission, IdentityAdmission},
    resource_access::ResourceOwner,
    SubjectId,
};
impl BotSetupService {
    pub(super) async fn execute(
        &self,
        event: SetupEvent,
        draft: SetupDraft,
        customization: SetupCustomization,
        request: Option<&SetupActionRequest>,
    ) -> Result<SetupView, BotError> {
        let mut setup = self.load(&event.actor).await?;
        if setup.actor != event.actor {
            return Err(BotError::Forbidden);
        }
        let request_digest = request.map(|request| request.digest(&event)).transpose()?;
        if let Some(request) = request {
            if request.replayed(&setup, request_digest.as_deref().unwrap())? {
                return self.finish_or_view(setup, None).await;
            }
            request.check_expected(&setup)?;
        }
        if request.is_none() && setup.processed_events.contains(&event.event_id) {
            return self.finish_or_view(setup, None).await;
        }
        let previous_revision = setup.revision;
        let previous_step = setup.step;
        let previous_grant = setup.grant_id.clone();
        let action = match request {
            Some(request) => request.choice(&setup)?,
            None => match &event.input {
                SetupInput::Interaction {
                    block_id,
                    action_id,
                } => {
                    if block_id != &setup.block_id() {
                        return self
                            .view(
                                &setup,
                                Some(
                                    "This choice has expired. Use the latest configuration.".into(),
                                ),
                                None,
                            )
                            .await;
                    }
                    Some(action_id.clone())
                }
                SetupInput::Text(text) => text_action(&setup, text),
            },
        };
        let signed_text = match &event.input {
            SetupInput::Text(text) => Some(text.as_str()),
            _ => None,
        };
        let text = if request.is_none() { signed_text } else { None };
        let mut connection_url = None;
        let mut notice = None;
        if action.as_deref() == Some("cancel")
            && !matches!(setup.step, SetupStep::Provisioning | SetupStep::Complete)
        {
            setup.step = SetupStep::Cancelled;
        } else if matches!(setup.step, SetupStep::Complete | SetupStep::Cancelled)
            && action.as_deref() == Some("new")
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
                match signed_text.and_then(login_proof) {
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
            if action.as_deref() == Some("edit")
                && !matches!(
                    setup.step,
                    SetupStep::Login
                        | SetupStep::Provisioning
                        | SetupStep::Complete
                        | SetupStep::Cancelled
                )
            {
                setup.step = SetupStep::Owner;
                setup.confirmation_digest = None;
            } else if let Some(action) = action.as_deref() {
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
                    SetupStep::Agent | SetupStep::Workspace | SetupStep::Sandbox => {
                        self.choose_component(&mut setup, action).await?;
                    }
                    SetupStep::AgentCustom => {
                        if action == "back" {
                            setup.step = SetupStep::Agent;
                        } else {
                            setup.configuration.agent.thinking_effort = match action {
                                "agent:quick" => crate::profiles::ThinkingEffort::Low,
                                "agent:thorough" => crate::profiles::ThinkingEffort::High,
                                "done" => setup.configuration.agent.thinking_effort,
                                _ => return Err(invalid_action()),
                            };
                            self.dependencies
                                .validate_configuration(&setup.configuration)
                                .await?;
                            setup.step = SetupStep::Workspace;
                        }
                    }
                    SetupStep::WorkspaceCustom => {
                        if action == "back" {
                            setup.step = SetupStep::Workspace;
                        } else {
                            match action {
                                "workspace:prepared" | "done" => {}
                                "workspace:clean" => {
                                    setup.configuration.workspace.setup_commands.clear()
                                }
                                _ => return Err(invalid_action()),
                            }
                            self.dependencies
                                .validate_configuration(&setup.configuration)
                                .await?;
                            setup.step = SetupStep::Sandbox;
                        }
                    }
                    SetupStep::SandboxCustom => {
                        if action == "back" {
                            setup.step = SetupStep::Sandbox;
                        } else {
                            match action {
                                "done" => {}
                                "sandbox:on_demand" => {
                                    setup.configuration.availability.always_keep_alive = false
                                }
                                "sandbox:always_live" => {
                                    setup.configuration.availability.always_keep_alive = true
                                }
                                _ => return Err(invalid_action()),
                            }
                            self.dependencies
                                .validate_configuration(&setup.configuration)
                                .await?;
                            setup.selected_services.clear();
                            setup.step = SetupStep::Services;
                        }
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
                            super::agent_actions::require_consent(
                                &setup,
                                &event,
                                setup
                                    .confirmation_digest
                                    .as_deref()
                                    .ok_or(BotError::Conflict)?,
                            )?;
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
            } else if let Some(request) = request {
                request.apply_content(&mut setup, &draft, &customization)?;
                self.dependencies
                    .validate_configuration(&setup.configuration)
                    .await?;
            } else if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                // An empty explicit setup command resumes the current view;
                // it is not a replacement profile or a confirmation.
                if super::custom_text::apply(&mut setup, text)? {
                    self.dependencies
                        .validate_configuration(&setup.configuration)
                        .await?;
                    notice = Some("Configuration updated. Continue editing or choose Done.".into());
                } else {
                    match setup.step {
                        SetupStep::Name if setup.draft.name.is_some() => {
                            setup.step = SetupStep::Agent
                        }
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
                            setup.configuration.sandbox =
                                customization.sandbox.ok_or_else(|| {
                                    BotError::Invalid(
                                        "Provide the customized Sandbox Profile configuration."
                                            .into(),
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
        }
        if request.is_some_and(|request| matches!(request.action, SetupAction::View))
            && setup.step == previous_step
            && setup.grant_id == previous_grant
            && (setup.step != SetupStep::Review || setup.confirmation_event.is_some())
        {
            return self.finish_or_view(setup, notice).await;
        }
        if setup.step == SetupStep::Review && setup.confirmation_event.is_none() {
            setup.confirmation_event = Some(SetupReviewEvent {
                event_id: event.event_id.clone(),
                occurred_at: event.occurred_at.clone(),
                reviewed_at: crate::now_rfc3339(),
            });
        } else if setup.step != SetupStep::Review && setup.step != SetupStep::Provisioning {
            setup.confirmation_event = None;
        }
        if let (Some(request), Some(digest)) = (request, request_digest) {
            setup.processed_actions.push(SetupActionReceipt {
                request_id: request.request_id.clone(),
                digest,
            });
            if setup.processed_actions.len() > 128 {
                setup.processed_actions.remove(0);
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
}
