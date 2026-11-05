//! Production control-plane service and router assembly.
use std::sync::Arc;
mod access;
mod adapter;
mod bindings;
mod bots;
mod credentials;
mod desktop;
mod documents;
mod event_routing;
mod media;
mod profiles;
mod routes;
mod runtime;
mod stores;
use crate::{
    activity::ActivityRecorder,
    auth::{service_portal, AuthState},
    config::Config,
    profile_authoring::CmaProfileAuthoringSource,
    profile_policy::profile_route_policy,
    projections::{AdapterAgentHistoryExporter, AdapterSandboxResources, AgentProjectionServices},
    rate_limit::RateLimiters,
    sandbox::{
        runtime_capabilities, LiveAgentRegistry, LiveSandboxPolicy, PlatformTitleEndpoint,
        SandboxLifecycleComposition, SandboxLifecycleConfig, SandboxLifecycleDependencies,
    },
    store::{DefaultProfileSpec, MongoStores},
    trigger::{CmaServiceConnectionBroker, TriggerComposition, TriggerCompositionDependencies},
};
use axum::Router;
use layer1_public::rest::{self, HierarchyState, ProfilesState};
use layer2_core::{
    profiles::authoring::ProfileAuthoringService,
    sandbox::{
        AgentContextService, HierarchyReadService, SandboxOperatorService, SandboxService,
        StoredSandboxRetirement, WorkspaceService,
    },
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};
mod state;
pub(crate) use state::ControlPlaneComposition;

impl ControlPlaneComposition {
    pub(crate) async fn build(config: &Config) -> Result<Self, String> {
        let stores = stores::connect(config).await?;
        let activity = ActivityService::new(stores.activity.clone());
        let documents = documents::compose(config, &stores)?;
        let users = UserDirectory::new(stores.users.clone());
        let profile_policy = profile_route_policy();
        let authoring_bounds = profile_policy.capacity_bounds;
        let default_capacity = layer3_adapter::SandboxCapacity {
            cpu_millis: config.default_profiles.cpu_millis,
            memory_mib: config.default_profiles.memory_mib,
        };
        let profile_scanner = Arc::clone(&profile_policy.public_value_scanner);
        profiles::seed_profiles(&stores, config, &profile_policy).await?;
        let retirement = Arc::new(StoredSandboxRetirement::new(stores.sandbox_model.clone()));
        let sandbox_operators = Arc::new(SandboxOperatorService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            retirement,
            Arc::clone(&activity),
        ));
        let settings = Arc::new(PlatformSettingsService::new(
            stores.settings.clone(),
            Arc::clone(&activity),
        ));
        let sandbox_policy = Arc::new(LiveSandboxPolicy::new(
            Arc::clone(&settings),
            config.baseline_services.clone(),
        ));
        let subscriptions =
            crate::connections::CodexSubscriptions::new(stores.codex_subscriptions.clone());
        let access_tools = credentials::agent_access_tools(config, &stores)?;
        let (provider_tools, provider_tool_routes) = credentials::provider_tools(config, &stores)?;
        let adapters = adapter::production_adapter(
            config,
            stores.sandbox_model.clone(),
            subscriptions.clone(),
            access_tools.clone(),
            provider_tools,
        )
        .await?;
        let adapter = adapters.owned;
        let auth =
            credentials::authentication(config, &stores, &users, &activity, settings.clone())?;
        let broker = Arc::new(
            CmaServiceConnectionBroker::from_auth_config_with_callback(
                &config.auth_mode,
                config.nyxid_public_url.as_deref(),
                Some(config.public_url.as_str()),
                Arc::clone(&auth),
            )
            .map_err(|error| format!("service connection broker is not usable: {error}"))?
            .with_subscriptions(subscriptions.clone()),
        );
        let connection_prerequisite = crate::connections::ConnectionPrerequisite::new(
            auth.clone(),
            broker.clone(),
            sandbox_policy.clone(),
        );
        let browser_connections = crate::connections::BrowserConnections::new(
            stores.clone(),
            sandbox_policy.clone(),
            broker.clone(),
            subscriptions,
        );
        let sandbox_service = SandboxService::with_policy_source(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            sandbox_policy.clone(),
            connection_prerequisite.clone(),
        );
        let workspace_service = WorkspaceService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
        )
        .with_connections(connection_prerequisite.clone());
        let live_agents = Arc::new(LiveAgentRegistry::default());
        let resource_access = bindings::resource_access(&stores, auth.clone());
        let agent_page_origin = layer1_public::agent_page::AgentPageOrigin::new(&config.public_url)
            .map_err(|error| error.to_string())?;
        let projections = AgentProjectionServices::new(
            agent_page_origin.clone(),
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            Arc::clone(&live_agents),
            Arc::new(AdapterSandboxResources::new(Arc::clone(&adapter))),
            runtime_capabilities(adapter.capabilities()),
        )
        .with_connections(connection_prerequisite.clone())
        .with_message_feedback(stores.message_feedback.clone())
        .with_suggestions(stores.sandbox_model.clone())
        .with_skill_support(Arc::new(crate::projections::AdapterAgentSkillSupport(
            adapter.clone(),
        )))
        .with_sessions(stores.sandbox_model.clone())
        .with_resource_access(resource_access.clone())
        .with_history_exporter(Arc::new(AdapterAgentHistoryExporter::new(adapter.clone())));
        let image_resolver = crate::agent_lifecycle::ImageManagerResolver::new(
            config.image_manager_base_url.clone(),
        )
        .map_err(|error| format!("Image Manager resolver is not usable: {error}"))?;
        let inline_lifecycle = crate::agent_lifecycle::InlineAgentLifecycle::new(
            Arc::clone(&sandbox_service),
            Arc::clone(&workspace_service),
            projections.agents(),
            stores.sandbox_model.clone(),
            agent_page_origin.clone(),
            stores.sandbox_profiles.clone(),
            image_resolver.clone(),
            authoring_bounds,
            config.agent_manager_sandbox_capacity,
            config.runtime.machine_sandbox_image.clone(),
            profile_scanner.clone(),
            stores.agent_lifecycle.clone(),
            Some(stores.sandbox_model.clone()),
            Some(adapter::desktop_creation(config, adapter.clone())?),
        )
        .with_task_experience(config.task_experience.clone());
        inline_lifecycle.resume_pending();
        let projections = projections
            .with_lifecycle(inline_lifecycle.clone())
            .with_lifecycle_operations(inline_lifecycle);
        let bindings = bindings::BindingComposition::new(
            &stores,
            layer2_core::sandbox::binding::BindingResources {
                sandboxes: sandbox_service.clone(),
                workspaces: workspace_service.clone(),
                agents: projections.agents(),
            },
            sandbox_policy.clone(),
            authoring_bounds,
        );
        let hierarchy = HierarchyReadService::new(stores.sandbox_model.clone())
            .with_resource_access(resource_access.clone());
        let runtime = runtime::RuntimeServices::new(config, &stores, adapter.clone());
        let native_warm_pools = runtime.warm_pools;
        let admin = layer1_public::rest::admin::AdminState::new(
            Arc::clone(&settings),
            sandbox_operators,
            Arc::clone(&activity),
        )
        .with_user_directory(Arc::clone(&users))
        .with_runtime_updates(runtime.updates.clone())
        .with_maintenance(stores.sandbox_model.clone())
        .with_warm_pools(native_warm_pools.clone());
        let operations: Arc<dyn OperationStore> = stores.operations.clone();
        let profiles = ProfilesState::new(
            stores.sandbox_profiles.clone(),
            Arc::clone(&operations),
            Arc::clone(&settings),
            profile_policy,
        );
        let hierarchy = HierarchyState::new(
            Arc::clone(&sandbox_service),
            Arc::clone(&workspace_service),
            hierarchy,
            Arc::clone(&operations),
        )
        .with_runtime_updates(runtime.updates)
        .with_user_directory(Arc::clone(&users))
        .with_sandbox_colors(layer2_core::sandbox::colors::SandboxColorService::new(
            stores.sandbox_colors.clone(),
            stores.sandbox_model.clone(),
            Arc::new(layer2_core::sandbox::colors::SystemColorRandom::new()),
        ));
        let recorder = ActivityRecorder::new(Arc::clone(&activity));
        let profile_authoring = ProfileAuthoringService::new(
            authoring_bounds,
            CmaProfileAuthoringSource::production(config, Arc::clone(&auth))?,
        );
        let sandbox_credentials = credentials::sandbox_credentials(config, auth.clone(), &stores);
        let model_routes = routes::models(
            config,
            projections.reads(),
            sandbox_credentials.clone(),
            adapter.clone(),
        )?;
        let agent_skills = credentials::agent_skills(config, sandbox_credentials.clone())?;
        let limits = RateLimiters::from_config(
            config.rate_limits,
            config.trusted_proxies.clone(),
            stores.request_limits.clone(),
        );
        let worker_holder = format!("{}:{}", config.deployment_id, uuid::Uuid::new_v4());
        let sandbox_lifecycle = SandboxLifecycleComposition::new(
            SandboxLifecycleDependencies {
                store: stores.sandbox_model.clone(),
                title_runtime: Arc::new(PlatformTitleEndpoint {
                    platform_base_url: config.runtime.platform_base_url.clone(),
                }),
                adapter: adapter.clone(),
                credentials: sandbox_credentials,
                registry: Arc::clone(&live_agents),
                workspaces: Arc::clone(&workspace_service),
                agents: projections.agents(),
                operations: projections.operations(),
                policy: sandbox_policy.clone(),
                skills: agent_skills,
                funding: runtime::funding(config, &stores, adapter.clone())?,
            },
            SandboxLifecycleConfig {
                access_transport_migration: config.agent_access_transport_migration.clone(),
                managed_documents_enabled: config.document_storage.is_some(),
                home_policy: config
                    .runtime
                    .home_policy
                    .clone()
                    .or_else(|| config.runtime.agent_sandbox_home_policy.clone()),
                execution: config.execution,
                holder: worker_holder.clone(),
                deployment_id: config.deployment_id.clone(),
                runtime_version: config.runtime.sandbox_image.clone(),
                title_model: config.default_profiles.agent_model.clone(),
            },
        );
        let trigger = TriggerComposition::new(TriggerCompositionDependencies {
            common_first_messages: config.features.trigger_command_admission,
            agent_reads: projections.reads(),
            credentials: connection_prerequisite.clone(),
            stores: Arc::clone(&stores),
            directory: users,
            settings: Arc::clone(&settings),
            broker: broker.clone(),
            sandboxes: Arc::clone(&sandbox_service),
            workspaces: Arc::clone(&workspace_service),
            agents: projections.agents(),
            operations: Arc::clone(&operations),
            rate_limiters: Arc::clone(&limits),
            public_origin: config.public_url.clone(),
            worker_holder: worker_holder.clone(),
            profile_bounds: authoring_bounds,
            capacity: default_capacity,
            profile_scanner: profile_scanner.clone(),
            image_resolver: image_resolver.clone(),
        })?;
        let (trigger_routes, launch_scheduler) = trigger.into_parts();
        let context = AgentContextService::new(
            sandbox_service,
            workspace_service,
            projections.agents(),
            stores.sandbox_profiles.clone(),
            settings.clone(),
        );
        let media = media::compose(
            config,
            &stores,
            resource_access.clone(),
            auth.clone(),
            adapter.clone(),
        )?;
        let command_scheduler = media::commands(
            crate::commands::ProductionCommandDependencies {
                source_guard: media::guard(&media),
                authority: auth.clone(),
                access: resource_access.clone(),
                store: stores.sandbox_model.clone(),
                operations: projections.operations(),
                context: context.clone(),
            },
            worker_holder.clone(),
        )?;
        let sandbox_control_scheduler = crate::commands::SandboxControlScheduler::new(
            layer2_core::sandbox::control::SandboxControlWorker::new(
                stores.sandbox_model.clone(),
                resource_access.clone(),
                auth.clone(),
                connection_prerequisite.clone(),
            ),
        );
        let sandbox_control_service = layer2_core::sandbox::control::SandboxControlService::new(
            stores.sandbox_model.clone(),
            resource_access.clone(),
            auth.clone(),
        );
        let deliverables = access::deliverables(
            &stores,
            projections.reads(),
            resource_access.clone(),
            documents.clone(),
            adapter.clone(),
        );
        let agent_access = access::compose(
            &stores,
            auth.clone(),
            sandbox_policy,
            broker,
            resource_access.clone(),
            worker_holder,
            access::ToolServices {
                source: access_tools,
                documents: config.document_storage.as_ref().map(|_| documents.clone()),
                deliverables: deliverables.clone(),
                public_origin: config.public_url.clone(),
                task_experience: config.task_experience.clone(),
            },
        );
        let command_service = agent_access.commands;
        let projections = projections.with_commands(command_service.clone());
        let responses_v2 = bindings::responses(
            &stores,
            command_service.clone(),
            projections.reads(),
            agent_page_origin,
            &bindings,
            config.gateway.as_ref(),
        )?;
        let event_workers = layer2_core::event_workers::EventWorkerProfileService::new(
            stores.event_workers.clone(),
            layer2_core::bots::PublishedBotProfiles::new(
                stores.sandbox_profiles.clone(),
                settings.clone(),
                config.default_profiles.agent_model.clone(),
            ),
            "cma_codex".into(),
        );
        let resource_routes = rest::profiles_router(profiles)
            .merge(rest::answer_deliverables_router(deliverables))
            .merge(media::router(media))
            .merge(rest::documents_router(documents.clone()))
            .merge(model_routes)
            .merge(agent_access.owner_routes)
            .merge(rest::event_workers::router(event_workers.clone()))
            .merge(routes::agent_files(
                stores.sandbox_model.clone(),
                resource_access.clone(),
                adapter.clone(),
                config.task_experience.clone(),
            ))
            .merge(layer1_public::rest::sandbox_commands_router(
                sandbox_control_service,
            ))
            .merge(routes::agent_metadata(&stores, resource_access.clone()))
            .merge(layer1_public::rest::resource_access_router(resource_access))
            .merge(rest::profile_authoring_router(profile_authoring))
            .merge(rest::hierarchy_router(hierarchy))
            .merge(projections.router(Arc::clone(&operations)))
            .merge(desktop::router(
                projections.reads(),
                adapter.clone(),
                config,
            ))
            .merge(projections.conversation_router(
                context,
                sandbox_lifecycle.updates(),
                &config.public_url,
            ))
            .merge(responses_v2)
            .merge(projections.agent_events_router(config.features, sandbox_lifecycle.updates()))
            .merge(trigger_routes)
            .merge(layer1_public::rest::admin::router(admin));
        let resource_routes = resource_routes.merge(bindings.router(config.gateway.as_ref())?);
        let (bot_routes, bots) = bots::BotComposition::build(bots::BotCompositionDependencies {
            config,
            stores: &stores,
            auth: auth.clone(),
            profiles: event_workers,
            image_resolver,
            connections: connection_prerequisite.clone(),
            profile_bounds: authoring_bounds,
            profile_scanner,
        })?;
        let (resource_routes, mut actor_continuations) = (resource_routes.merge(bot_routes), None);
        let resource_routes = if config.bot_father.is_some() {
            let bot_service = bots
                .as_ref()
                .ok_or("Bot Father requires Bot management")?
                .service
                .clone();
            let dependencies = crate::bot_setups::SetupDependencies::build(
                config,
                stores.clone(),
                auth.clone(),
                settings.clone(),
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
        let provenance_backfill =
            crate::sandbox::AgentProvenanceBackfill::new(stores.sandbox_model.clone());
        let usage = crate::usage_billing::UsageWorkers::new(&stores, config)?;
        let usage_metrics = crate::sandbox_metrics::MetricsWorkers::new(config).await?;
        let resource_routes = resource_routes.merge(usage_metrics.router());
        let tool_routes = agent_access.tool_routes.merge(provider_tool_routes);
        let router = routes::compose(
            resource_routes,
            browser_connections,
            auth.clone(),
            recorder,
            limits,
            stores,
            tool_routes.merge(service_portal::router(
                auth.clone(),
                config.gateway.as_ref(),
            )?),
        );
        Ok(Self {
            router,
            auth,
            bindings,
            bots,
            actor_continuations,
            provenance_backfill,
            usage,
            usage_metrics,
            launch_scheduler,
            command_scheduler,
            sandbox_control_scheduler,
            sandbox_lifecycle,
            native_warm_pools,
            native_prepared: adapters.preparation,
            agent_access_worker: agent_access.worker,
        })
    }
}
