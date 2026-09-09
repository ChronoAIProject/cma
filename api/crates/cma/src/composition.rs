//! Production control-plane service and router assembly.

use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};
use layer1_public::rest::{
    hierarchy_router, profile_authoring_router, profiles_router, HierarchyState, ProfilesState,
};
use layer2_core::{
    profiles::authoring::ProfileAuthoringService,
    sandbox::{
        AgentContextService, HierarchyReadService, SandboxOperatorService, SandboxService,
        StoredSandboxRetirement, WorkspaceService,
    },
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};

use crate::{
    activity::ActivityRecorder,
    auth::{keys::KeyIssuer, AuthMode, AuthState, SignInRecords},
    config::Config,
    profile_authoring::CmaProfileAuthoringSource,
    profile_policy::profile_route_policy,
    projections::{AdapterSandboxResources, AgentProjectionServices},
    rate_limit::RateLimiters,
    sandbox::{
        runtime_capabilities, DurableCheckpointObjectPurger, LiveAgentRegistry, LiveSandboxPolicy,
        ManagedSandboxCredentials, OrnnAgentSkillResolver, ProviderTitleEndpoint,
        SandboxCredentialController, SandboxLifecycleComposition, SandboxLifecycleConfig,
        SandboxLifecycleDependencies, UnavailableSandboxCredentials,
    },
    store::{DefaultProfileSpec, MongoStores},
    trigger::{CmaServiceConnectionBroker, TriggerComposition, TriggerCompositionDependencies},
    REQUEST_BODY_LIMIT_BYTES,
};

pub(crate) struct ControlPlaneComposition {
    pub router: Router,
    pub launch_scheduler: super::trigger::LaunchScheduler,
    pub sandbox_lifecycle: SandboxLifecycleComposition,
}

impl ControlPlaneComposition {
    pub(crate) async fn build(config: &Config) -> Result<Self, String> {
        let checkpoint_storage = crate::store::ChronoStorage::new(
            &config.runtime.platform_base_url,
            &config.checkpoint_storage,
        )
        .map_err(|error| format!("checkpoint storage is not usable: {error}"))?;
        let stores = Arc::new(
            MongoStores::connect(
                &config.mongodb_uri,
                &config.mongodb_database,
                &config.deployment_id,
                &config.auth_encryption_key,
                config.activity_retention,
                checkpoint_storage,
                &config.runtime.sandbox_image,
            )
            .await
            .map_err(|error| format!("durable stores are not usable: {error}"))?,
        );
        let activity = ActivityService::new(stores.activity.clone());
        let users = UserDirectory::new(stores.users.clone());
        let profile_policy = profile_route_policy();
        let authoring_bounds = profile_policy.capacity_bounds;
        stores
            .sandbox_profiles
            .seed_default_profiles(
                &DefaultProfileSpec {
                    cpu_millis: config.default_profiles.cpu_millis,
                    memory_mib: config.default_profiles.memory_mib,
                    agent_model: config.default_profiles.agent_model.clone(),
                    agent_effort: config.default_profiles.agent_effort,
                },
                profile_policy.capacity_bounds,
                profile_policy.public_value_scanner.as_ref(),
            )
            .await
            .map_err(|error| format!("default profiles are not usable: {error}"))?;

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
        let adapter = production_adapter(config, Arc::clone(&stores.checkpoint_objects)).await?;
        let auth = AuthState::with_store_and_nyxid_public_url(
            config.auth_mode.clone(),
            Some(SignInRecords {
                users: Arc::clone(&users),
                activity: Arc::clone(&activity),
            }),
            stores.auth.clone(),
            config.nyxid_public_url.clone(),
        );
        let sandbox_service = SandboxService::with_policy_source(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            sandbox_policy.clone(),
            auth.clone(),
        );
        let workspace_service = WorkspaceService::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
        );
        let live_agents = Arc::new(LiveAgentRegistry::default());
        let projections = AgentProjectionServices::new(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            Arc::clone(&live_agents),
            Arc::new(AdapterSandboxResources::new(Arc::clone(&adapter))),
            runtime_capabilities(adapter.capabilities()),
        );
        let hierarchy = HierarchyReadService::new(stores.sandbox_model.clone());
        let admin = layer1_public::rest::admin::AdminState::new(
            Arc::clone(&settings),
            sandbox_operators,
            Arc::clone(&activity),
        )
        .with_user_directory(Arc::clone(&users));
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
        .with_user_directory(Arc::clone(&users));
        let recorder = ActivityRecorder::new(Arc::clone(&activity));
        let profile_authoring = ProfileAuthoringService::new(
            authoring_bounds,
            CmaProfileAuthoringSource::production(config, Arc::clone(&auth))?,
        );
        let sandbox_credentials: Arc<dyn SandboxCredentialController> = match &config.auth_mode {
            AuthMode::Provider { base_url, .. } => ManagedSandboxCredentials::new(
                Arc::new(KeyIssuer::new(
                    reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(15))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("the rustls HTTP client builds"),
                    base_url,
                    &config.deployment_id,
                )),
                Arc::clone(&auth),
                Arc::clone(&stores.sandbox_credentials),
            ),
            AuthMode::Trusted { .. } => Arc::new(UnavailableSandboxCredentials),
        };
        if !auth.requires_credential() {
            tracing::warn!(
                "running with a trusted fixed subject; this mode is for local development only"
            );
        }
        let agent_skills = match &config.auth_mode {
            AuthMode::Provider { .. } => OrnnAgentSkillResolver::scoped(
                &config.runtime.platform_base_url,
                Arc::clone(&sandbox_credentials),
            )?,
            AuthMode::Trusted { .. } => {
                OrnnAgentSkillResolver::anonymous(&config.runtime.platform_base_url)?
            }
        };

        let limits = RateLimiters::from_config(config.rate_limits, config.trusted_proxies.clone());
        let worker_holder = format!("{}:{}", config.deployment_id, uuid::Uuid::new_v4());
        let sandbox_lifecycle = SandboxLifecycleComposition::new(
            SandboxLifecycleDependencies {
                checkpoint_objects: Arc::clone(&stores.checkpoint_objects),
                store: stores.sandbox_model.clone(),
                title_runtime: Arc::new(ProviderTitleEndpoint {
                    adapter: Arc::clone(&adapter),
                    client: layer3_adapter::provider::SandboxClient::new(
                        config.runtime.sandbox_base_url.clone(),
                        config.runtime.sandbox_api_key.clone(),
                    ),
                    deployment_id: config.deployment_id.clone(),
                }),
                adapter,
                credentials: sandbox_credentials,
                checkpoints: Arc::new(DurableCheckpointObjectPurger::new(
                    stores.checkpoint_objects.clone(),
                )),
                registry: Arc::clone(&live_agents),
                workspaces: Arc::clone(&workspace_service),
                agents: projections.agents(),
                operations: projections.operations(),
                policy: sandbox_policy,
                skills: agent_skills,
            },
            SandboxLifecycleConfig {
                execution: config.execution,
                holder: worker_holder.clone(),
                deployment_id: config.deployment_id.clone(),
                runtime_version: config.runtime.sandbox_image.clone(),
                title_model: config.default_profiles.agent_model.clone(),
            },
        );
        let broker = Arc::new(
            CmaServiceConnectionBroker::from_auth_config(
                &config.auth_mode,
                config.nyxid_public_url.as_deref(),
                Arc::clone(&auth),
            )
            .map_err(|error| format!("service connection broker is not usable: {error}"))?,
        );
        let trigger = TriggerComposition::new(TriggerCompositionDependencies {
            agent_reads: projections.reads(),
            credentials: auth.clone(),
            stores: Arc::clone(&stores),
            directory: users,
            settings: Arc::clone(&settings),
            broker,
            sandboxes: Arc::clone(&sandbox_service),
            workspaces: Arc::clone(&workspace_service),
            agents: projections.agents(),
            operations: Arc::clone(&operations),
            rate_limiters: Arc::clone(&limits),
            public_origin: config.public_url.clone(),
            worker_holder,
        })?;
        let (trigger_routes, launch_scheduler) = trigger.into_parts();
        let context = AgentContextService::new(
            sandbox_service,
            workspace_service,
            projections.agents(),
            stores.sandbox_profiles.clone(),
            settings,
        );
        let resource_routes = profiles_router(profiles)
            .merge(profile_authoring_router(profile_authoring))
            .merge(hierarchy_router(hierarchy))
            .merge(projections.router(Arc::clone(&operations)))
            .merge(projections.conversation_router(context, sandbox_lifecycle.updates()))
            .merge(trigger_routes)
            .merge(layer1_public::rest::admin::router(admin));
        let public = super::rest_router(resource_routes)
            .merge(crate::auth::router(Arc::clone(&auth)))
            .layer(axum::middleware::from_fn_with_state(
                recorder,
                crate::activity::record,
            ))
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&limits),
                crate::rate_limit::enforce,
            ))
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ));
        let router = Router::new()
            .merge(public)
            .merge(crate::health::router(stores))
            .layer(axum::middleware::from_fn_with_state(
                limits,
                crate::rate_limit::admit,
            ))
            .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT_BYTES));

        Ok(Self {
            router,
            launch_scheduler,
            sandbox_lifecycle,
        })
    }
}

async fn production_adapter(
    config: &Config,
    checkpoint_objects: Arc<crate::store::MongoCheckpointObjectStore>,
) -> Result<Arc<dyn layer3_adapter::SandboxAdapter>, String> {
    use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
    use layer3_adapter::hosted::{
        CheckpointArchiveStoreFactory, HostedDeployment, ProviderHostedBackend,
    };
    use layer3_adapter::provider::{BatchSandboxReclaimer, SandboxClient};

    let reclaimer =
        BatchSandboxReclaimer::infer_namespaced(config.runtime.sandbox_namespace.clone())
            .await
            .map_err(|error| format!("sandbox cleanup authority is not usable: {error}"))?
            .for_creator(config.deployment_id.clone());
    reclaimer
        .preflight_cleanup_authority()
        .await
        .map_err(|error| format!("sandbox cleanup authority is not readable: {error}"))?;
    let client = SandboxClient::new(
        config.runtime.sandbox_base_url.clone(),
        config.runtime.sandbox_api_key.clone(),
    )
    .with_reclaimer(reclaimer);
    let archive_factory: Arc<dyn CheckpointArchiveStoreFactory> = checkpoint_objects;
    let backend = ProviderHostedBackend::production(client, archive_factory)
        .map_err(|error| format!("hosted runtime transport is not usable: {error}"))?;
    let mut deployment = HostedDeployment::default();
    deployment.image.uri = config.runtime.sandbox_image.clone();
    deployment.upstream_base_url = config.runtime.upstream_base_url.clone();
    deployment.upstream_wire_api = config.runtime.upstream_wire_api.clone();
    deployment.platform_base_url = config.runtime.platform_base_url.clone();
    deployment
        .contract()
        .map_err(|error| format!("hosted runtime image is not usable: {error}"))?;
    Ok(Arc::new(CodexAdapter::with_backend(
        backend,
        CodexSandboxSpec {
            deployment,
            ..CodexSandboxSpec::default()
        },
    )))
}
