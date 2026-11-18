use super::*;
use crate::agent_manager::relay::{PublicAccess, Relay};
use crate::auth::test_resources as resources;
use crate::sandbox::LiveAgentRegistry;
use layer2_core::{
    agent_ownership::{candidates, Authority, Member, OwnershipCoordinator, PROTOCOL_VERSION},
    profiles::store::InMemoryProfileStore,
    resource_access::ResourceAccessService,
    sandbox::{
        AgentOperationService, AgentReadService, AgentRuntimeConnection, AgentRuntimeError,
        AgentRuntimeRegistry, AgentRuntimeTarget, AgentService, DurableAgentUpdateRecorder,
        RuntimeBinding, SandboxResourceReadError, SandboxResourceReader, SandboxResourceReading,
    },
};
use std::sync::Mutex;

pub(super) struct Registry {
    live: LiveAgentRegistry,
    pub cancel: Mutex<Option<Arc<Authority>>>,
    pub admitted_without_owner: Mutex<Vec<bool>>,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Registry {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        AgentRuntimeRegistry::resolve(&self.live, target).await
    }
    async fn fence(&self, id: &AgentId, fence: u64) -> Result<(), AgentRuntimeError> {
        self.admitted_without_owner
            .lock()
            .unwrap()
            .push(layer2_core::agent_ownership::execution_owner().is_none());
        AgentRuntimeRegistry::fence(&self.live, id, fence).await?;
        if let Some(authority) = self.cancel.lock().unwrap().as_ref() {
            authority.revoke();
        }
        Ok(())
    }
}
struct Resources;
#[async_trait::async_trait]
impl SandboxResourceReader for Resources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("durable control admission cannot query runtime resources")
    }
}

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: Arc<MongoSandboxStore>,
    pub ownership: Arc<MongoAgentOwnership>,
    pub coordinators: Vec<Arc<OwnershipCoordinator>>,
    pub routers: Vec<axum::Router>,
    pub registry: Arc<Registry>,
    pub identity: identity::Identity,
}
impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI required");
        assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
        let client = Client::with_uri_str(uri).await.unwrap();
        let name = format!("cma_public_admission_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let ownership = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
        let operations = Arc::new(
            crate::store::MongoOperationStore::open(&client, &name)
                .await
                .unwrap(),
        );
        let registry = Arc::new(Registry {
            live: LiveAgentRegistry::default(),
            cancel: Mutex::default(),
            admitted_without_owner: Mutex::default(),
        });
        let identity = identity::Identity::new().await;
        let mut coordinators = Vec::new();
        let mut routers = Vec::new();
        for index in 0..2 {
            let coordinator = OwnershipCoordinator::register(
                ownership.clone(),
                Member {
                    instance_id: format!("admission-{index}"),
                    protocol: PROTOCOL_VERSION,
                    relay_address: "http://127.0.0.1:1".into(),
                    expires_at_ms: 0,
                    draining: false,
                    owned_agents: 0,
                    max_owned_agents: 256,
                },
            )
            .await
            .unwrap();
            let updates = DurableAgentUpdateRecorder::new(store.clone());
            let relay = Relay::new(
                coordinator.clone(),
                &updates,
                ring::hmac::Key::new(ring::hmac::HMAC_SHA256, uuid::Uuid::new_v4().as_bytes()),
            )
            .unwrap();
            let access = Arc::new(ResourceAccessService::new(
                store.clone(),
                identity.auth.clone(),
            ));
            let reads = AgentReadService::new(store.clone(), Arc::new(Resources))
                .with_resource_access(access.clone());
            let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
            relay
                .install_public_access(PublicAccess::new(
                    identity.auth.clone(),
                    reads.clone(),
                    access,
                    Arc::new(adapter),
                ))
                .unwrap();
            let agents = AgentService::new(
                store.clone(),
                Arc::new(InMemoryProfileStore::default()),
                registry.clone(),
                Default::default(),
            );
            let routes =
                layer1_public::rest::agent_router(layer1_public::rest::AgentRoutesState::new(
                    layer1_public::agent_page::AgentPageOrigin::new("http://localhost").unwrap(),
                    agents,
                    reads,
                    AgentOperationService::new(store.clone(), registry.clone()),
                    operations.clone(),
                ));
            // Use the same auth/relay ordering as composition::routes::compose.
            // No AuthenticatedCaller or resource proof is injected by this fixture.
            routers.push(
                relay
                    .authenticated_routes(routes, identity.auth.clone())
                    .layer(axum::extract::DefaultBodyLimit::max(
                        crate::REQUEST_BODY_LIMIT_BYTES,
                    )),
            );
            coordinators.push(coordinator);
        }
        Self {
            client,
            name,
            store,
            ownership,
            coordinators,
            routers,
            registry,
            identity,
        }
    }

    pub async fn seed(&self, suffix: &str, stopped: bool) -> Agent {
        let sandbox_id = format!("sbx-{suffix}");
        let workspace_id = format!("wsp-{suffix}");
        let worktree_id = format!("wkt-{suffix}");
        let agent_id = format!("agt-{suffix}");
        let mut sandbox = resources::sandbox(&sandbox_id, "owner");
        sandbox.runtime = Some(RuntimeBinding {
            boot_id: None,
            runtime_id: format!("runtime-{suffix}"),
            runtime_version: "fixture-image".into(),
            provision_generation: Some(1),
            bound_at: super::super::NOW.into(),
        });
        sandbox.lifecycle.runtime_generation_high_water = 1;
        self.store.create_sandbox(sandbox).await.unwrap();
        self.store
            .create_workspace(resources::workspace(&workspace_id, &sandbox_id, "owner"))
            .await
            .unwrap();
        self.store
            .create_worktree(resources::worktree(
                &worktree_id,
                &workspace_id,
                &sandbox_id,
                &agent_id,
                "owner",
            ))
            .await
            .unwrap();
        let mut agent = resources::agent(
            &agent_id,
            &worktree_id,
            &workspace_id,
            &sandbox_id,
            "owner",
            AgentStatus::Ready,
        );
        agent.conversation.conversation_id = Some(format!("conversation-{suffix}"));
        agent.conversation.runtime_generation = Some(1);
        if stopped {
            agent
                .transition(
                    AgentStatus::Stopped,
                    super::super::NOW.into(),
                    Some(AgentStopReason::Owner),
                )
                .unwrap();
        }
        crate::store::sandbox_maintenance::mongo_integration_tests::insert_agent(
            &self.store,
            &agent,
            super::super::NOW,
        )
        .await;
        agent
    }

    pub async fn acquire(&self, id: &AgentId) -> (usize, Arc<Authority>) {
        let selected = candidates(id, &self.ownership.membership().await.unwrap(), None).unwrap()
            [0]
        .instance_id
        .clone();
        let index = self
            .coordinators
            .iter()
            .position(|c| c.instance_id == selected)
            .unwrap();
        (
            index,
            self.coordinators[index].acquire(id).await.unwrap().unwrap(),
        )
    }

    pub async fn request(
        &self,
        ingress: usize,
        method: Method,
        path: &str,
        token: Option<&str>,
        input: (Option<&str>, &str),
        proof: Option<&str>,
    ) -> axum::response::Response {
        let (key, body) = input;
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if let Some(proof) = proof {
            request = request.header("x-cma-agent-manager-relay", proof);
        }
        self.routers[ingress]
            .clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    pub async fn stored(&self, id: &AgentId) -> Agent {
        self.store.get_agent(id).await.unwrap().unwrap()
    }
    pub async fn close(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
