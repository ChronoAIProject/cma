use super::*;
use layer2_core::profiles::{SandboxProfileSnapshot, WorkspaceLocation, WorkspaceProfileSnapshot};
use layer2_core::resource_access::{CredentialContext, CredentialSource, ResourceActor};
use layer2_core::sandbox::store::{InMemorySandboxStore, SandboxStore, WorkspaceStore};
use layer2_core::sandbox::*;
use layer2_core::{RuntimeCapabilities, RuntimeCapability, ServiceId};

const NOW: &str = "2026-10-13T00:00:00Z";
struct Registry;
#[async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Err(AgentRuntimeError::NotFound)
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

pub(super) fn context(subject: &str) -> CallerContext {
    CallerContext {
        caller: Caller::new(
            SubjectId::new(subject),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "fixture".into(),
            subject: SubjectId::new(subject),
            credential_id: "original-credential".into(),
        },
        credential_source: CredentialSource::Independent { expires_at: None },
        actor: ResourceActor::Api {
            subject: SubjectId::new(subject),
            delegation_id: None,
        },
        memberships: vec![],
    }
}

pub(super) fn facade(
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<crate::store::MongoProfileStore>,
    operations: Arc<MongoAgentLifecycleStore>,
) -> Arc<InlineAgentLifecycle> {
    facade_with_limit(
        store,
        profiles,
        operations,
        SandboxPolicy::default().max_sandboxes_per_owner,
        FixedSandboxCredentialPrerequisite::SetupRequired,
    )
}

pub(crate) async fn creation_test_router(
    database: &mongodb::Database,
    operations: Arc<MongoAgentLifecycleStore>,
) -> (axum::Router, tokio::sync::OwnedSemaphorePermit) {
    let store = Arc::new(InMemorySandboxStore::default());
    let profiles = Arc::new(
        crate::store::MongoProfileStore::open(database.client(), database.name())
            .await
            .unwrap(),
    );
    let service = facade(store.clone(), profiles.clone(), operations);
    let slots = service
        .worker_slots
        .clone()
        .acquire_many_owned(crate::store::DISCOVERY_BATCH as u32)
        .await
        .unwrap();
    let routes = creation_routes(database, store, profiles, service).await;
    (routes, slots)
}

pub(super) async fn creation_routes(
    database: &mongodb::Database,
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<crate::store::MongoProfileStore>,
    service: Arc<InlineAgentLifecycle>,
) -> axum::Router {
    let records = Arc::new(
        crate::store::MongoOperationStore::open_for_test(database.client(), database.name())
            .await
            .unwrap(),
    );
    let mut owner = context("owner");
    owner.caller.authorization =
        Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]);
    crate::projections::AgentProjectionServices::new(
        service.agent_page_origin.clone(),
        store,
        profiles,
        Arc::new(Registry),
        Arc::new(crate::projections::UnavailableSandboxResources),
        RuntimeCapabilities::new([RuntimeCapability::Conversation]),
    )
    .with_lifecycle(service.clone())
    .with_lifecycle_operations(service)
    .router(records)
    .layer(axum::Extension(layer1_public::AuthenticatedSubject(
        owner.caller.subject.clone(),
    )))
    .layer(axum::Extension(owner.caller.authorization.clone()))
    .layer(axum::Extension(owner))
}

pub(super) fn facade_with_limit(
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<crate::store::MongoProfileStore>,
    operations: Arc<MongoAgentLifecycleStore>,
    limit: u16,
    credentials: impl layer2_core::sandbox::SandboxCredentialPrerequisite + 'static,
) -> Arc<InlineAgentLifecycle> {
    facade_with_resolver(
        store,
        profiles,
        operations,
        limit,
        credentials,
        Arc::new(RetiredImageResolver),
    )
}

pub(super) fn facade_with_resolver(
    store: Arc<InMemorySandboxStore>,
    profiles: Arc<crate::store::MongoProfileStore>,
    operations: Arc<MongoAgentLifecycleStore>,
    limit: u16,
    credentials: impl layer2_core::sandbox::SandboxCredentialPrerequisite + 'static,
    resolver: Arc<dyn ImageResolver>,
) -> Arc<InlineAgentLifecycle> {
    InlineAgentLifecycle::new(
        SandboxService::new(
            store.clone(),
            profiles.clone(),
            SandboxPolicy {
                max_sandboxes_per_owner: limit,
                ..SandboxPolicy::default()
            },
            Arc::new(credentials),
        ),
        WorkspaceService::new(store.clone(), profiles.clone()),
        AgentService::new(
            store.clone(),
            profiles.clone(),
            Arc::new(Registry),
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        ),
        store,
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        profiles,
        resolver,
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        layer3_adapter::SandboxCapacity::default(),
        None,
        Arc::new([] as [String; 0]),
        operations,
        None,
        None,
    )
}

pub(in crate::agent_lifecycle) async fn seed(store: &InMemorySandboxStore) -> Workspace {
    let sandbox = Sandbox {
        id: SandboxId("sbx_existing".into()),
        owner: SubjectId::new("owner"),
        name: "Existing".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_existing".into()),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 1024,
                nyxid_services: vec![
                    ServiceId::new("github"),
                    ServiceId::new("chrono-llm-public"),
                    ServiceId::new("ornn-api"),
                ],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: vec![],
            service_scope: None,
            reaches: vec![
                ServiceId::new("github"),
                ServiceId::new("chrono-llm-public"),
                ServiceId::new("ornn-api"),
            ],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    };
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let workspace = Workspace {
        id: WorkspaceId("wk_existing".into()),
        sandbox_id: sandbox.id,
        owner: sandbox.owner,
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_existing".into()),
            revision: 1,
            digest: "b".repeat(64),
            content: WorkspaceProfileContent {
                location: WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    store.create_workspace(workspace.clone()).await.unwrap();
    workspace
}

pub(super) fn task_policy() -> Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy> {
    Arc::new(
        serde_json::from_value(json!({
            "mode":"accounts", "accounts":["owner"], "capabilities":["task_home_admission"]
        }))
        .unwrap(),
    )
}
