//! Production composition of the Trigger REST and Launch worker surfaces.

use std::sync::Arc;

use axum::Router;
use layer1_public::{
    agui::{launch_companion_router, LaunchCompanionState},
    rest::{
        trigger_assets_router, trigger_router, LaunchQuota, TriggerAssetsState, TriggerRoutesState,
    },
};
use layer2_core::{
    profiles::{ProfileCapacityBounds, PublicValueScanner},
    sandbox::{
        AgentReadService, AgentService, SandboxCredentialPrerequisite, SandboxService,
        WorkspaceService,
    },
    trigger_domain::{
        LaunchService, ServiceConnectionBroker, ServiceConnectionService, TriggerService,
    },
    OperationStore, PlatformSettingsService, UserDirectory,
};

use super::InlineTriggerProfileMaterializer;
use super::{
    scheduler::LaunchScheduler,
    worker::{
        LaunchWorker, LaunchWorkerConfig, ServiceLaunchResourceDependencies,
        ServiceLaunchResourceDriver,
    },
    CmaServiceConnectionBroker, LiveLaunchPolicy,
};
use crate::agent_lifecycle::ImageResolver;
use crate::{rate_limit::CmaLaunchQuota, rate_limit::RateLimiters, store::MongoStores};

pub(crate) struct TriggerCompositionDependencies {
    pub common_first_messages: bool,
    pub standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    pub credentials: Arc<dyn SandboxCredentialPrerequisite>,
    pub stores: Arc<MongoStores>,
    pub directory: Arc<UserDirectory>,
    pub settings: Arc<PlatformSettingsService>,
    pub broker: Arc<CmaServiceConnectionBroker>,
    pub sandboxes: Arc<SandboxService>,
    pub workspaces: Arc<WorkspaceService>,
    pub agents: Arc<AgentService>,
    pub agent_reads: Arc<AgentReadService>,
    pub operations: Arc<dyn OperationStore>,
    pub rate_limiters: Arc<RateLimiters>,
    pub public_origin: String,
    pub worker_holder: String,
    pub profile_bounds: ProfileCapacityBounds,
    pub capacity: layer3_adapter::SandboxCapacity,
    pub profile_scanner: Arc<dyn PublicValueScanner>,
    pub image_resolver: Arc<dyn ImageResolver>,
}

pub(crate) struct TriggerComposition {
    routes: Router,
    scheduler: LaunchScheduler,
}

impl TriggerComposition {
    pub(crate) fn new(dependencies: TriggerCompositionDependencies) -> Result<Self, String> {
        let definitions = dependencies.stores.trigger_definitions.clone();
        let profiles = dependencies.stores.sandbox_profiles.clone();
        let resources = dependencies.stores.sandbox_model.clone();
        let launch_policy = Arc::new(LiveLaunchPolicy::new(Arc::clone(&dependencies.settings)));
        let inline_materializer = InlineTriggerProfileMaterializer::new(
            dependencies.stores.sandbox_profiles.clone(),
            dependencies.profile_bounds,
            dependencies.capacity,
            dependencies.profile_scanner,
            dependencies.image_resolver,
        );
        let triggers = TriggerService::new_with_inline_materializer(
            definitions.clone(),
            definitions.clone(),
            dependencies.directory,
            inline_materializer,
        )
        .with_account_standing(dependencies.standing.clone());
        let launches = LaunchService::new(
            definitions.clone(),
            profiles.clone(),
            definitions.clone(),
            resources.clone(),
            dependencies.broker.clone(),
            launch_policy,
            dependencies.credentials,
        )
        .with_common_first_messages(dependencies.common_first_messages)
        .with_account_standing(dependencies.standing.clone());
        let connections =
            ServiceConnectionService::new(definitions.clone(), dependencies.broker.clone());
        let launch_quota: Arc<dyn LaunchQuota> = Arc::new(CmaLaunchQuota::new(
            dependencies.rate_limiters,
            Arc::clone(&dependencies.settings),
        ));
        let route_state = TriggerRoutesState::new(
            triggers.clone(),
            launches,
            connections,
            dependencies.operations,
            launch_quota,
        );
        let assets = TriggerAssetsState::new(definitions.clone(), &dependencies.public_origin)
            .map_err(|error| format!("Trigger public origin is not usable: {error}"))?
            .with_account_standing(dependencies.standing);
        let companion = LaunchCompanionState::new(definitions.clone());
        let routes = trigger_router(route_state)
            .merge(layer1_public::rest::agent_source_router(
                dependencies.agent_reads,
                triggers,
            ))
            .merge(trigger_assets_router(assets))
            .merge(launch_companion_router(companion));

        let broker: Arc<dyn ServiceConnectionBroker> = dependencies.broker;
        let driver = ServiceLaunchResourceDriver::new(ServiceLaunchResourceDependencies {
            definitions,
            profiles,
            resources,
            settings: dependencies.settings,
            connections: broker,
            sandboxes: dependencies.sandboxes,
            workspaces: dependencies.workspaces,
            agents: dependencies.agents,
        });
        let worker = LaunchWorker::new(
            dependencies.stores.trigger_definitions.clone(),
            driver,
            LaunchWorkerConfig::new(dependencies.worker_holder),
        );
        Ok(Self {
            routes,
            scheduler: LaunchScheduler::new(worker),
        })
    }

    pub(crate) fn into_parts(self) -> (Router, LaunchScheduler) {
        (self.routes, self.scheduler)
    }
}
