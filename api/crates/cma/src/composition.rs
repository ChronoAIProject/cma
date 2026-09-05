//! Production control-plane service and router assembly.

use std::sync::Arc;

use axum::{extract::DefaultBodyLimit, Router};
use layer1_public::rest::{hierarchy_router, profiles_router, HierarchyState, ProfilesState};
use layer2_core::{
    sandbox::{
        HierarchyReadService, SandboxOperatorService, SandboxService, StoredSandboxRetirement,
        WorkspaceService,
    },
    ActivityService, OperationStore, PlatformSettingsService, UserDirectory,
};

use crate::{
    activity::ActivityRecorder,
    auth::{keys::KeyIssuer, AuthMode, AuthState, SignInRecords},
    config::Config,
    profile_policy::profile_route_policy,
    projections::{AgentProjectionServices, UnavailableSandboxResources},
    rate_limit::RateLimiters,
    sandbox::{
        runtime_capabilities, LiveAgentRegistry, LiveSandboxPolicy, ManagedSandboxCredentials,
    },
    store::{DefaultProfileSpec, MongoStores},
    trigger::{CmaServiceConnectionBroker, TriggerComposition, TriggerCompositionDependencies},
    REQUEST_BODY_LIMIT_BYTES,
};

pub(crate) struct ControlPlaneComposition {
    pub router: Router,
    pub launch_scheduler: super::trigger::LaunchScheduler,
}

impl ControlPlaneComposition {
    pub(crate) async fn build(config: &Config) -> Result<Self, String> {
        let stores = Arc::new(
            MongoStores::connect(
                &config.mongodb_uri,
                &config.mongodb_database,
                &config.deployment_id,
                &config.auth_encryption_key,
                config.activity_retention,
            )
            .await
            .map_err(|error| format!("durable stores are not usable: {error}"))?,
        );
        let activity = ActivityService::new(stores.activity.clone());
        let users = UserDirectory::new(stores.users.clone());
        let profile_policy = profile_route_policy();
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
                &profile_policy.secret_patterns,
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
        let sandbox_service = SandboxService::with_policy_source(
            stores.sandbox_model.clone(),
            stores.sandbox_profiles.clone(),
            sandbox_policy,
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
            Arc::new(UnavailableSandboxResources),
            runtime_capabilities(layer3_adapter::RuntimeContract::V3.capabilities()),
        );
        let hierarchy = HierarchyReadService::new(stores.sandbox_model.clone());
        let admin = layer1_public::rest::admin::AdminState::new(
            Arc::clone(&settings),
            sandbox_operators,
            Arc::clone(&activity),
        );
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
        );
        let recorder = ActivityRecorder::new(Arc::clone(&activity));
        let auth = AuthState::with_store_and_nyxid_public_url(
            config.auth_mode.clone(),
            Some(SignInRecords {
                users: Arc::clone(&users),
                activity: Arc::clone(&activity),
            }),
            stores.auth.clone(),
            config.nyxid_public_url.clone(),
        );
        let _sandbox_credentials = match &config.auth_mode {
            AuthMode::Provider { base_url, .. } => Some(ManagedSandboxCredentials::new(
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
            )),
            AuthMode::Trusted { .. } => None,
        };
        if !auth.requires_credential() {
            tracing::warn!(
                "running with a trusted fixed subject; this mode is for local development only"
            );
        }

        let limits = RateLimiters::from_config(config.rate_limits, config.trusted_proxies.clone());
        let broker = Arc::new(
            CmaServiceConnectionBroker::from_auth_config(
                &config.auth_mode,
                config.nyxid_public_url.as_deref(),
                Arc::clone(&auth),
            )
            .map_err(|error| format!("service connection broker is not usable: {error}"))?,
        );
        let trigger = TriggerComposition::new(TriggerCompositionDependencies {
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
            worker_holder: format!("{}:{}", config.deployment_id, uuid::Uuid::new_v4()),
        })?;
        let (trigger_routes, launch_scheduler) = trigger.into_parts();
        let resource_routes = profiles_router(profiles)
            .merge(hierarchy_router(hierarchy))
            .merge(projections.router(Arc::clone(&operations)))
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
        })
    }
}
