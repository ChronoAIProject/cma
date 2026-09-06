use layer2_core::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
use layer2_core::sandbox::{AgentContextService, SandboxPolicy, SandboxService, WorkspaceService};
use layer2_core::PlatformDefaults;

use super::*;

pub(crate) async fn conversation_fixture(
) -> (crate::AgentConversationState, Arc<InMemorySandboxStore>) {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let sandbox = sandbox("sbx_route", OWNER);
    let workspace = workspace("wks_route", "sbx_route", OWNER);
    let defaults = PlatformDefaults {
        sandbox_profile: SandboxProfileRef::new(
            sandbox.profile.id.clone(),
            sandbox.profile.revision,
        ),
        workspace_profile: WorkspaceProfileRef::new(
            workspace.profile.id.clone(),
            workspace.profile.revision,
        ),
        agent_profile: AgentProfileRef::new(AgentProfileId("agp_route".into()), 1),
    };
    store.create_sandbox(sandbox).await.unwrap();
    store.create_workspace(workspace).await.unwrap();
    publish_profile(&profiles).await;
    let registry = TestRegistry::new();
    let agents = AgentService::new(
        store.clone(),
        profiles.clone(),
        registry.clone(),
        agent_capabilities(),
    );
    let reads = AgentReadService::new(store.clone(), Arc::new(TestResources));
    let operations = AgentOperationService::new(store.clone(), registry);
    let context = AgentContextService::new(
        SandboxService::new(store.clone(), profiles.clone(), SandboxPolicy::default()),
        WorkspaceService::new(store.clone(), profiles.clone()),
        agents,
        profiles,
        Arc::new(defaults),
    );
    (
        crate::AgentConversationState::new(reads, operations, context),
        store,
    )
}
