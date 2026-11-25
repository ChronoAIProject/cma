use std::sync::Arc;
mod access;
mod account_deletion;
pub(crate) mod activity;
mod adapter;
mod agent_manager;
pub(crate) mod backend_free;
mod bindings;
mod bots;
mod capacity_recovery;
mod credentials;
mod desktop;
mod documents;
pub(crate) mod event_routing;
mod legal_consent;
mod media;
mod observation;
mod profiles;
mod routes;
mod runtime;
mod runtime_inventory;
mod stores;
mod usage;
use crate::{
    auth::{service_portal, AuthState},
    config::Config,
    profile_policy::profile_route_policy,
    projections::{AdapterAgentHistoryExporter, AdapterSandboxResources, AgentProjectionServices},
    sandbox::{
        runtime_capabilities, LiveAgentRegistry, LiveSandboxPolicy, PlatformTitleEndpoint,
        SandboxLifecycleComposition, SandboxLifecycleDependencies,
    },
    store::{DefaultProfileSpec, MongoStores},
    trigger::{TriggerComposition, TriggerCompositionDependencies},
};
use layer1_public::rest::{self, HierarchyState, ProfilesState};
use layer2_core::{
    sandbox::{AgentContextService, HierarchyReadService, SandboxService, WorkspaceService},
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};
mod state;
pub(crate) use state::ControlPlaneComposition;
impl ControlPlaneComposition {
    async fn assemble(config: &Config) -> Result<Self, String> {
        let stores = stores::connect(config).await?;
        let (deletions, standing) = account_deletion::compose(config, &stores).await?;
        let activity = ActivityService::new(stores.activity.clone());
        let documents::DocumentComposition {
            service: documents,
            maintenance: document_blob_maintenance,
        } = documents::compose(config, &stores, standing.clone())?;
        let users = UserDirectory::new(stores.users.clone());
        let profile_policy = profile_route_policy();
        let authoring_bounds = profile_policy.capacity_bounds;
        let profile_scanner = Arc::clone(&profile_policy.public_value_scanner);
        profiles::seed_profiles(&stores, config, &profile_policy).await?;
        let sandbox_operators = access::sandbox_operators(&stores, &activity);
        let settings = profiles::platform_settings(config, &stores, &activity);
        let sandbox_policy = Arc::new(LiveSandboxPolicy::new(
            Arc::clone(&settings),
            config.baseline_services.clone(),
        ));
        let subscriptions =
            crate::connections::CodexSubscriptions::new(stores.codex_subscriptions.clone());
        let access_tools = credentials::agent_access_tools(config, &stores, standing.clone())?;
        let (provider_tools, provider_tool_routes) =
            credentials::provider_tools(config, &stores, standing.clone())?;
        let adapters = adapter::production_adapter(
            config,
            stores.sandbox_model.clone(),
            subscriptions.clone(),
            access_tools.clone(),
            provider_tools,
            standing.clone(),
        )
        .await?;
        let adapter = adapters.owned;
        let auth =
            credentials::authentication(config, &stores, &users, &activity, settings.clone())?
                .with_account_standing(standing.clone())
                .with_account_deletion_enabled(config.account_deletion_enabled)
                .with_privacy_export_enabled(config.features.privacy_export_enabled);
        let credentials::Connections {
            broker,
            browser: browser_connections,
            prerequisite: connection_prerequisite,
        } = credentials::connections(
            config,
            auth.clone(),
            subscriptions,
            &stores,
            sandbox_policy.clone(),
        )?;
        let sandbox_service = SandboxService::with_policy_source(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            sandbox_policy.clone(),
            connection_prerequisite.clone(),
        );
        sandbox_service.attach_platform_spend(stores.sandbox_model.clone());
        let workspace_service = WorkspaceService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
        )
        .with_connections(connection_prerequisite.clone());
        let live_agents = Arc::new(LiveAgentRegistry::default());
        let agent_manager = agent_manager::register(config, &stores, &live_agents).await?;
        let ownership = agent_manager
            .as_ref()
            .map(|manager| manager.coordinator.clone());
        let resource_access = bindings::resource_access(&stores, auth.clone(), standing.clone());
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
        .with_features(config.features, adapters.model_funding.clone())
        .with_connections(connection_prerequisite.clone())
        .with_message_feedback(stores.message_feedback.clone())
        .with_suggestions(stores.sandbox_model.clone())
        .with_skill_support(Arc::new(crate::projections::AdapterAgentSkillSupport(
            adapter.clone(),
        )))
        .with_sessions(stores.sandbox_model.clone())
        .with_resource_access(resource_access.clone())
        .with_response_logs(stores.sandbox_model.response_log_reader())
        .with_history_exporter(Arc::new(AdapterAgentHistoryExporter::new(adapter.clone())));
        let (projections, observations) =
            observation::prepare(config, &stores, &agent_manager, projections).await?;
        let image_resolver = crate::agent_lifecycle::ImageManagerResolver::new(
            config.image_manager_base_url.clone(),
            config.features.image_manager_retirement,
        )?;
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
            Some(adapter::desktop_creation(config, adapter.clone(), &stores)?),
        )
        .with_released_runtime(stores.released_runtime.clone())
        .with_task_experience(config.task_experience.clone())
        .with_session_guidance_v4(config.features.session_guidance_v4)
        .with_failure_metadata(config.lifecycle_failure_metadata_enabled)
        .with_managed_repository_preflight(config.features.github_repository_preflight);
        let lifecycle_drain = inline_lifecycle.drain_handle();
        if let Some(manager) = &agent_manager {
            manager.register_lifecycle(lifecycle_drain.clone())?;
        }
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
            .with_detail_timings(config.features.sandbox_detail_timings)
            .with_resource_access(resource_access.clone());
        let runtime = runtime::RuntimeServices::new(config, &stores, adapter.clone());
        let native_warm_pools = runtime.warm_pools;
        let admin = runtime_inventory::admin_state(
            config.features,
            Arc::clone(&settings),
            sandbox_operators,
            Arc::clone(&activity),
            Arc::clone(&adapter),
        )
        .with_user_directory(Arc::clone(&users))
        .with_runtime_updates(runtime.updates.clone())
        .with_maintenance(stores.sandbox_model.clone())
        .with_warm_pools(native_warm_pools.clone())
        .with_platform_spend(stores::platform_spend(&stores, &activity, &settings))
        .with_admission_ceilings(stores::admission_ceilings(&stores, config))
        .with_admission_control(Arc::new(
            layer2_core::admission::AdmissionControlService::new(stores.admission_control.clone())
                .with_writes_enabled(config.features.admission_control_writes_enabled),
        ));
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
        .with_concurrent_detail_enrichment(config.features.concurrent_sandbox_detail_enrichment)
        .with_runtime_updates(runtime.updates)
        .with_retained_recovery(runtime.recovery)
        .with_retained_reopening(runtime.reopen)
        .with_user_directory(Arc::clone(&users))
        .with_sandbox_colors(layer2_core::sandbox::colors::SandboxColorService::new(
            stores.sandbox_colors.clone(),
            stores.sandbox_model.clone(),
            Arc::new(layer2_core::sandbox::colors::SystemColorRandom::new()),
        ));
        let (recorder, activity_worker) = activity::compose(config, &activity, &stores);
        let profile_authoring = profiles::authoring_router(config, &auth, authoring_bounds)?;
        let sandbox_credentials = credentials::sandbox_credentials(config, auth.clone(), &stores);
        let model_routes = routes::models(
            config,
            projections.reads(),
            sandbox_credentials.clone(),
            adapter.clone(),
        )?;
        let agent_skills = credentials::agent_skills(config, sandbox_credentials.clone())?;
        let limits = crate::control_admission::limiters(config, stores.request_limits.clone());
        let worker_holder = format!("{}:{}", config.deployment_id, uuid::Uuid::new_v4());
        let mut sandbox_lifecycle = SandboxLifecycleComposition::new(
            SandboxLifecycleDependencies {
                store: stores.sandbox_model.clone(),
                pending_homes: Arc::new(stores.admission_ceilings.pending_homes.clone()),
                title_budget: stores.request_limits.clone(),
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
            runtime::lifecycle_config(config, worker_holder.clone(), ownership.clone(), &stores),
        )?;
        if let (Some(manager), Some(capture)) =
            (&agent_manager, sandbox_lifecycle.capture_drain.take())
        {
            manager.register_retirement_capture(capture)?;
        }
        sandbox_lifecycle.register_inline_lifecycle(lifecycle_drain);
        let trigger = TriggerComposition::new(TriggerCompositionDependencies {
            standing: standing.clone(),
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
            capacity: layer3_adapter::SandboxCapacity {
                cpu_millis: config.default_profiles.cpu_millis,
                memory_mib: config.default_profiles.memory_mib,
            },
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
        let cancel_wake = (config.features.command_cancel_wake && ownership.is_some())
            .then(|| Arc::new(tokio::sync::Notify::new()));
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
            ownership.clone(),
            config.features.command_reconcile_pacing,
            adapters.model_funding.clone(),
        )?
        .with_cancel_wake(cancel_wake.clone());
        let (sandbox_control_scheduler, sandbox_control_service) = access::sandbox_control(
            &stores,
            resource_access.clone(),
            auth.clone(),
            connection_prerequisite.clone(),
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
                ownership,
                cancel_wake,
                model_credit_required: config.features.model_credit_required,
                model_funding: adapters.model_funding,
                source: access_tools,
                documents: config.document_storage.as_ref().map(|_| documents.clone()),
                deliverables: deliverables.clone(),
                public_origin: config.public_url.clone(),
                task_experience: config.task_experience.clone(),
            },
        );
        let projections = projections.with_commands(agent_access.commands.clone());
        let responses_v2 = bindings::responses(
            &stores,
            agent_access.commands.clone(),
            projections.reads(),
            agent_page_origin,
            &bindings,
            config.gateway.as_ref(),
            config.features,
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
        let metadata = routes::agent_metadata(&stores, resource_access.clone(), config);
        let accounts = routes::accounts(config, &stores, deletions, standing, projections.reads());
        let resource_routes = rest::profiles_router(profiles)
            .merge(accounts)
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
                config,
            ))
            .merge(rest::sandbox_commands_router(sandbox_control_service))
            .merge(metadata)
            .merge(capacity_recovery::router(
                &stores,
                resource_access.clone(),
                &mut sandbox_lifecycle,
                config.features.capacity_recovery_enabled,
            ))
            .merge(rest::resource_access_router(resource_access.clone()))
            .merge(profile_authoring)
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
        let resource_routes = routes::stream_budget(resource_routes, config.public_stream_limit);
        let resource_routes = crate::control_admission::routes(resource_routes, limits.lifecycle());
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
        let bots::BotSetupComposition {
            router: resource_routes,
            actor_continuations,
        } = bots::compose_setup(
            config,
            stores.clone(),
            auth.clone(),
            settings.clone(),
            bots.as_ref(),
            agent_access.commands,
            resource_routes.merge(bot_routes),
        )?;
        let provenance_backfill =
            crate::sandbox::AgentProvenanceBackfill::new(stores.sandbox_model.clone());
        let usage = crate::usage_billing::UsageWorkers::new(&stores, config)?;
        let usage_metrics =
            crate::sandbox_metrics::MetricsWorkers::new(config, adapters.runtime_capacity).await?;
        let resource_routes = resource_routes.merge(usage_metrics.router());
        let tool_routes = agent_access.tool_routes.merge(provider_tool_routes);
        let worker_observations = runtime::worker_observations(config, &stores);
        let workers = crate::worker_supervision::Workers::new();
        let relay = agent_manager::relay(
            config,
            &agent_manager,
            &sandbox_lifecycle,
            stores.sandbox_model.clone(),
            adapters.owner_controls,
            (
                auth.clone(),
                projections.reads(),
                resource_access,
                adapter.clone(),
            ),
        )?;
        observations.finish(&relay, &sandbox_lifecycle, &projections)?;
        let router = routes::compose(
            resource_routes,
            browser_connections,
            auth.clone(),
            recorder,
            (limits, config),
            (
                crate::health::router_with_workers(stores.clone(), workers.clone()),
                tool_routes.merge(service_portal::router(
                    auth.clone(),
                    config.gateway.as_ref(),
                )?),
            ),
            relay.as_ref(),
        );
        Ok(Self {
            activity_worker,
            workers,
            document_blob_maintenance,
            worker_observations,
            router: agent_manager::internal_router(&relay, &agent_manager, router),
            agent_manager,
            agent_relay: relay,
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
