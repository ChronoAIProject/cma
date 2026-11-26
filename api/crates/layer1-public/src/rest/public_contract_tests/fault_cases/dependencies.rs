use std::sync::Arc;

use layer2_core::profiles::{InMemoryProfileStore, ProfileCapacityBounds, ProfileVisibility};
use layer2_core::sandbox::{
    AgentOperationService, AgentReadService, AgentService, HierarchyReadService,
    SandboxOperatorService, SandboxPolicy, SandboxService, StoredSandboxRetirement,
    WorkspaceService,
};
use layer2_core::{ActivityService, Authorization, Capability, PlatformSettingsService};

use super::super::super::{
    admin, agent_router, agents_tests as agents, hierarchy_router, instances_tests as instances,
    profiles_router, AgentRoutesState, HierarchyState, ProfileRoutePolicy, ProfilesState,
};
use super::super::Scenario;
use super::instance_store::UnavailableInstances;
use super::profile_store::UnavailableProfiles;
use super::settings_store::{UnavailableActivity, UnavailableSettings};

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if name == "conversation_dependency_unavailable" {
        let (mut state, _) = agents::conversation_fixture().await;
        state.reads = AgentReadService::new(
            Arc::new(UnavailableInstances),
            Arc::new(agents::TestResources),
        );
        return Some(Scenario {
            router: crate::pty::router(state.reads.clone())
                .merge(crate::agui::router(state.clone()))
                .merge(crate::responses::router(state)),
            subject: Some("owner-a"),
            authorization: Authorization::new([Capability::UseOwnSandboxes]),
            symbols: Default::default(),
        });
    }
    if !matches!(
        name,
        "instances_dependency_unavailable" | "settings_dependency_unavailable"
    ) {
        return None;
    }
    let store = Arc::new(UnavailableInstances);
    let profiles = Arc::new(InMemoryProfileStore::default());
    let operations = Arc::new(instances::MemoryOperationStore::default());
    let activity = ActivityService::new(Arc::new(UnavailableActivity));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(UnavailableSettings),
        activity.clone(),
    ));
    let router = if name == "settings_dependency_unavailable" {
        let operators = Arc::new(SandboxOperatorService::new(
            store.clone(),
            Arc::new(UnavailableProfiles),
            Arc::new(StoredSandboxRetirement::new(store)),
            activity.clone(),
        ));
        admin::router(admin::AdminState::new(
            settings.clone(),
            operators,
            activity,
        ))
        .merge(profiles_router(ProfilesState::new(
            profiles,
            operations,
            settings,
            ProfileRoutePolicy {
                capacity_bounds: ProfileCapacityBounds {
                    min_cpu_millis: 250,
                    max_cpu_millis: 4_000,
                    min_memory_mib: 256,
                    max_memory_mib: 8_192,
                },
                public_value_scanner: Arc::new([]),
            },
        )))
    } else {
        instances::publish_sandbox_profile(
            &profiles,
            "sbp_contract",
            "owner-a",
            ProfileVisibility::Private,
            vec![],
        )
        .await;
        let registry = Arc::new(agents::TestRegistry);
        hierarchy_router(HierarchyState::new(
            SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
            ),
            WorkspaceService::new(store.clone(), profiles.clone()),
            HierarchyReadService::new(store.clone()),
            operations.clone(),
        ))
        .merge(agent_router(AgentRoutesState::new(
            crate::agent_page::AgentPageOrigin::test(),
            AgentService::new(
                store.clone(),
                profiles,
                registry.clone(),
                agents::agent_capabilities(),
            ),
            AgentReadService::new(store.clone(), Arc::new(agents::TestResources)),
            AgentOperationService::new(store, registry),
            operations,
        )))
    };
    Some(Scenario {
        router,
        subject: Some("owner-a"),
        authorization: Authorization::new([
            Capability::ManageOwnProfiles,
            Capability::CreateSandbox,
            Capability::UseOwnSandboxes,
            Capability::DeleteOwnSandbox,
            Capability::ConfigurePlatform,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
            Capability::ReadAllSandboxes,
            Capability::ControlAnySandbox,
            Capability::ReadEventLog,
            Capability::ReadRuntimeInventory,
        ]),
        symbols: Default::default(),
    })
}
