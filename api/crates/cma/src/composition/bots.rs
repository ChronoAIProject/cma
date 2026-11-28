use super::*;
use crate::agent_lifecycle::ImageResolver;
use crate::auth::AuthMode;
use axum::Router;
use layer1_public::rest::{AgentLifecycleImageReference, AgentLifecycleProvisioningError};
use layer2_core::bots::BotError;
use layer2_core::bots::{BotControl, BotService, BotWorker};
use layer2_core::profiles::{
    AgentProfileContent, AgentProvider, ModelAccess, PermissionLevel,
    PrivateProfileCreationService, ProfileCapacityBounds, SandboxProfileContent, SkillRef,
    SkillSource, ThinkingEffort, WorkspaceLocation, WorkspaceProfileContent,
};
use layer2_core::{
    bots::{BotInlineProfileResolver, BotProfiles, CreateBot},
    profiles::WorkspaceProfileRef,
    sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError},
    SubjectId,
};
use std::sync::Arc;

pub(crate) struct BotComposition {
    pub(super) service: Arc<BotService>,
    worker: BotWorker,
    activity: Arc<ActivityService>,
}

struct InlineBotProfileResolver {
    profiles: Arc<PrivateProfileCreationService>,
    image_resolver: Arc<dyn ImageResolver>,
    connections: Arc<dyn SandboxCredentialPrerequisite>,
    capacity: layer3_adapter::SandboxCapacity,
    default_model: String,
}

#[async_trait::async_trait]
impl BotInlineProfileResolver for InlineBotProfileResolver {
    async fn resolve_inline(
        &self,
        owner: &SubjectId,
        request: &CreateBot,
        delegated_credential: Option<&str>,
        key: &str,
    ) -> Result<BotProfiles, BotError> {
        let inline = request
            .inline_agent
            .as_ref()
            .ok_or_else(|| BotError::Invalid("inline agent configuration is required".into()))?;
        let thinking_effort = match inline.reasoning_effort.as_deref().unwrap_or("high") {
            "minimal" => ThinkingEffort::Minimal,
            "low" => ThinkingEffort::Low,
            "medium" => ThinkingEffort::Medium,
            "high" => ThinkingEffort::High,
            "x_high" => ThinkingEffort::XHigh,
            value => {
                return Err(BotError::Invalid(format!(
                    "unsupported reasoning effort {value}"
                )))
            }
        };
        let permission_level = match inline.permission_mode.as_deref().unwrap_or("autonomous") {
            "guarded" => PermissionLevel::Guarded,
            "autonomous" => PermissionLevel::Autonomous,
            value => {
                return Err(BotError::Invalid(format!(
                    "unsupported permission level {value}"
                )))
            }
        };
        let skills = inline
            .skills
            .iter()
            .map(|skill| {
                if skill.id.trim().is_empty() || skill.id.len() > 256 {
                    return Err(BotError::Invalid("invalid skill id".into()));
                }
                Ok(SkillRef {
                    source: SkillSource::Ornn,
                    id: layer2_core::SkillId::new(skill.id.clone()),
                    version: skill.version.clone(),
                })
            })
            .collect::<Result<Vec<_>, BotError>>()?;
        let agent_content = AgentProfileContent {
            isolated_agent: false,
            model: Some(
                inline
                    .model
                    .clone()
                    .unwrap_or_else(|| self.default_model.clone()),
            ),
            thinking_effort,
            permission_level,
            skills,
            system_prompt: inline.system_prompt.clone().unwrap_or_default(),
        };
        agent_content
            .validate()
            .map_err(|error| BotError::Invalid(error.to_string()))?;
        let runtime_image = match &request.image {
            Some(image) => Some(
                self.image_resolver
                    .resolve(
                        delegated_credential,
                        &AgentLifecycleImageReference {
                            id: image.id.clone(),
                            version: image.version.clone(),
                        },
                    )
                    .await
                    .map_err(image_error)?,
            ),
            None => None,
        };
        // Before Bot admission there are no runtime effects. A deployment
        // default/catalog change gets new immutable materialization identities;
        // already admitted Bots replay their original stored profiles upstream.
        let key = layer2_core::profiles::content_digest(&(
            key,
            &runtime_image,
            self.capacity,
            &agent_content,
            crate::config::REQUIRED_BASELINE_SERVICES,
        ))
        .map_err(|_| BotError::Unavailable)?;
        let agent = self
            .profiles
            .create_agent_for_owner(
                owner,
                &format!("event-gateway:{key}:agent"),
                request.name.clone(),
                "Inline Event Gateway consumer Agent configuration".into(),
                agent_content,
            )
            .await
            .map_err(profile_error)?;
        let location = match &request.repository {
            Some(repository) => WorkspaceLocation::GitRepository {
                url: repository.url.clone(),
                reference: repository.reference.clone(),
            },
            None => WorkspaceLocation::Directory {
                name: "workspace".into(),
            },
        };
        let workspace_content = WorkspaceProfileContent {
            location,
            name: if request.repository.is_some() {
                String::new()
            } else {
                "workspace".into()
            },
            setup_commands: Vec::new(),
            agent_profiles: Vec::new(),
        };
        let workspace = self
            .profiles
            .create_workspace_for_owner(
                owner,
                &format!("event-gateway:{key}:workspace"),
                format!("{} workspace", request.name),
                "Inline Event Gateway workspace".into(),
                workspace_content,
            )
            .await
            .map_err(profile_error)?;
        let sandbox_content = SandboxProfileContent {
            runtime_image,
            cpu_millis: self.capacity.cpu_millis,
            memory_mib: self.capacity.memory_mib,
            nyxid_services: inline
                .permissions
                .services
                .iter()
                .cloned()
                .chain(
                    crate::config::REQUIRED_BASELINE_SERVICES
                        .iter()
                        .map(|id| (*id).to_owned()),
                )
                .map(layer2_core::ServiceId::new)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            agent_provider: AgentProvider::Codex,
            model_access: ModelAccess::ChronoaiManaged,
            max_workspaces: 16,
            max_agents_per_workspace: 16,
            workspace_profiles: vec![WorkspaceProfileRef {
                id: workspace.profile_id.clone(),
                revision: workspace.revision,
            }],
            software_install_commands: Vec::new(),
            environment_variables: Vec::new(),
        };
        let required_services = sandbox_content.connection_services();
        let sandbox = self
            .profiles
            .create_sandbox_for_owner(
                owner,
                &format!("event-gateway:{key}:sandbox"),
                format!("{} sandbox", request.name),
                "Inline Event Gateway sandbox configuration".into(),
                sandbox_content,
            )
            .await
            .map_err(profile_error)?;
        self.connections
            .check_services(owner, &required_services)
            .await
            .map_err(|error| connection_error(error, &sandbox))?;
        Ok(BotProfiles {
            event_worker_profile: None,
            resolved_model: agent
                .content
                .model
                .clone()
                .unwrap_or_else(|| self.default_model.clone()),
            agent,
            sandbox,
            workspace,
        })
    }
}

fn image_error(error: AgentLifecycleProvisioningError) -> BotError {
    match error {
        AgentLifecycleProvisioningError::Template(error) => BotError::Invalid(error.to_string()),
        AgentLifecycleProvisioningError::Forbidden
        | AgentLifecycleProvisioningError::ImageAccessDenied => BotError::Forbidden,
        AgentLifecycleProvisioningError::SignInRequired => {
            BotError::Invalid("sign in again to authorize the selected image".into())
        }
        AgentLifecycleProvisioningError::ImageSelectionUnsupported => {
            BotError::ImageSelectionUnsupported
        }
        AgentLifecycleProvisioningError::Invalid(error)
        | AgentLifecycleProvisioningError::DesktopUnsupported(error)
        | AgentLifecycleProvisioningError::Conflict(error) => BotError::Invalid(error),
        AgentLifecycleProvisioningError::SandboxLimitReached { .. }
        | AgentLifecycleProvisioningError::AdmissionPaused
        | AgentLifecycleProvisioningError::HomeCapacity
        | AgentLifecycleProvisioningError::ConnectionRequired { .. }
        | AgentLifecycleProvisioningError::RepositoryAccessRequired { .. }
        | AgentLifecycleProvisioningError::Unavailable => BotError::Unavailable,
    }
}

fn connection_error(
    error: SandboxCredentialPrerequisiteError,
    sandbox: &layer2_core::profiles::SandboxProfileRevision,
) -> BotError {
    match error {
        SandboxCredentialPrerequisiteError::ConnectionsRequired { services } => {
            BotError::ConnectionRequired {
                services: services
                    .into_iter()
                    .map(|service| service.to_string())
                    .collect(),
                connection_target: serde_json::json!({
                    "type": "sandbox_profile",
                    "profile": {
                        "id": sandbox.profile_id.to_string(),
                        "revision": sandbox.revision,
                    },
                }),
            }
        }
        SandboxCredentialPrerequisiteError::SetupRequired => BotError::ConnectionRequired {
            services: vec!["cma-credentials".into()],
            connection_target: serde_json::json!({
                "type": "sandbox_profile",
                "profile": {
                    "id": sandbox.profile_id.to_string(),
                    "revision": sandbox.revision,
                },
            }),
        },
        SandboxCredentialPrerequisiteError::Unavailable(_) => BotError::Unavailable,
    }
}

fn profile_error(error: layer2_core::profiles::ProfileCreationError) -> BotError {
    match error {
        layer2_core::profiles::ProfileCreationError::Forbidden => BotError::Forbidden,
        layer2_core::profiles::ProfileCreationError::Conflict => BotError::Conflict,
        layer2_core::profiles::ProfileCreationError::Invalid(error) => {
            BotError::Invalid(error.to_string())
        }
        layer2_core::profiles::ProfileCreationError::Store(_) => BotError::Unavailable,
    }
}
pub(super) struct BotCompositionDependencies<'a> {
    pub config: &'a Config,
    pub stores: &'a MongoStores,
    pub auth: Arc<AuthState>,
    pub profiles: Arc<layer2_core::event_workers::EventWorkerProfileService>,
    pub image_resolver: Arc<dyn ImageResolver>,
    pub connections: Arc<dyn SandboxCredentialPrerequisite>,
    pub profile_bounds: ProfileCapacityBounds,
    pub profile_scanner: Arc<dyn layer2_core::profiles::PublicValueScanner>,
}

impl BotComposition {
    pub(super) fn build(
        dependencies: BotCompositionDependencies<'_>,
    ) -> Result<(Router, Option<Self>), String> {
        let BotCompositionDependencies {
            config,
            stores,
            auth,
            profiles,
            image_resolver,
            connections,
            profile_bounds,
            profile_scanner,
        } = dependencies;
        let Some(gateway) = &config.gateway else {
            return Ok((Router::new(), None));
        };
        let AuthMode::Provider { base_url, .. } = &config.auth_mode else {
            return Err("CMAEG bots require NyxID authentication".into());
        };
        let external =
            crate::gateway::GatewayBots::new(gateway, auth, base_url, &config.deployment_id)?;
        let inline_profiles = Arc::new(InlineBotProfileResolver {
            profiles: PrivateProfileCreationService::new(
                stores.sandbox_profiles.clone(),
                profile_bounds,
                profile_scanner,
            ),
            image_resolver,
            connections,
            capacity: layer3_adapter::SandboxCapacity {
                cpu_millis: config.default_profiles.cpu_millis,
                memory_mib: config.default_profiles.memory_mib,
            },
            default_model: config.default_profiles.agent_model.clone(),
        });
        let service = BotService::new(stores.bots.clone(), profiles, "cma_codex".into())
            .with_effect_reservations(config.features.bot_effect_reservations)
            .with_inline_profiles(inline_profiles);
        let router = rest::bot_router(BotControl::new(service.clone(), external.clone()));
        let worker = BotWorker::new(
            service.clone(),
            external,
            format!("{}:bots:{}", config.deployment_id, uuid::Uuid::new_v4()),
        );
        Ok((
            router,
            Some(Self {
                service,
                worker,
                activity: ActivityService::new(stores.activity.clone()),
            }),
        ))
    }
    pub(crate) fn spawn(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut after = None;
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(1));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = crate::shutdown::signalled(shutdown.clone()) => break, _ = ticks.tick() => {} }
                let pass = async {
                    let rows = self.service.pending(after.as_deref(), 100).await?;
                    after = if rows.len() == 100 {
                        rows.last().map(|b| b.bot_id.clone())
                    } else {
                        None
                    };
                    for row in rows {
                        if !due(&row) {
                            continue;
                        }
                        let outcome = if let Err(error) = self.worker.run_due(&row.bot_id).await {
                            // Only bounded error enums and internal Bot IDs; no opaque references.
                            tracing::warn!(bot_id = %row.bot_id, %error, "bot work will retry");
                            layer2_core::ActivityOutcome::Failed("bot_work_retry".into())
                        } else {
                            layer2_core::ActivityOutcome::Succeeded
                        };
                        let mut record = layer2_core::ActivityRecord::new(
                            layer2_core::ActivityAction::Update,
                            outcome,
                        )
                        .on(layer2_core::ActivityTarget::Resource(row.bot_id));
                        record.actor = Some(row.owner);
                        if self.activity.record(&record).await.is_err() {
                            tracing::warn!("bot work activity could not be persisted");
                        }
                    }
                    Ok::<_, layer2_core::bots::BotError>(())
                };
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    result = pass => { if result.is_err() { tracing::warn!("bot work discovery will retry"); } }
                }
            }
        })
    }
}

fn due(bot: &layer2_core::bots::Bot) -> bool {
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap_or_default();
    if let Some(work) = &bot.work {
        if work.effect.is_some() {
            return false;
        }
        work.expires_at
            .as_deref()
            .and_then(layer2_core::seconds_from_rfc3339)
            .is_none_or(|expiry| expiry <= now)
    } else {
        bot.resources
            .credentials
            .expires_at
            .as_deref()
            .and_then(layer2_core::seconds_from_rfc3339)
            .is_some_and(|expiry| expiry <= now.saturating_add(3 * 86400))
    }
}

#[cfg(test)]
#[path = "bots_tests.rs"]
mod tests;

pub(super) struct BotSetupComposition {
    pub(super) router: Router,
    pub(super) actor_continuations: Option<Arc<crate::bot_setups::ActorContinuationWorker>>,
}

pub(super) fn compose_setup(
    config: &Config,
    stores: Arc<MongoStores>,
    auth: Arc<AuthState>,
    settings: Arc<PlatformSettingsService>,
    bots: Option<&BotComposition>,
    command_service: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    resource_routes: Router,
) -> Result<BotSetupComposition, String> {
    let mut actor_continuations = None;
    let resource_routes = if config.bot_father.is_some() {
        let bot_service = bots
            .ok_or("Bot Father requires Bot management")?
            .service
            .clone();
        let dependencies = crate::bot_setups::SetupDependencies::build(
            config,
            stores,
            auth,
            settings,
            bot_service,
        )?;
        let service = dependencies.service();
        actor_continuations = Some(crate::bot_setups::ActorContinuationWorker::new(
            dependencies.clone(),
            command_service,
            resource_routes.clone(),
        ));
        let delegation_routes = dependencies.delegation_router(resource_routes.clone());
        resource_routes
            .merge(rest::bot_setup_router(service.clone()))
            .merge(delegation_routes)
            .layer(axum::Extension(service))
    } else {
        resource_routes
    };
    Ok(BotSetupComposition {
        router: resource_routes,
        actor_continuations,
    })
}

pub(super) fn event_worker_profiles(
    config: &Config,
    stores: &MongoStores,
    settings: &Arc<PlatformSettingsService>,
) -> Arc<layer2_core::event_workers::EventWorkerProfileService> {
    layer2_core::event_workers::EventWorkerProfileService::new(
        stores.event_workers.clone(),
        layer2_core::bots::PublishedBotProfiles::new(
            stores.sandbox_profiles.clone(),
            Arc::clone(settings),
            config.default_profiles.agent_model.clone(),
        ),
        "cma_codex".into(),
    )
}
