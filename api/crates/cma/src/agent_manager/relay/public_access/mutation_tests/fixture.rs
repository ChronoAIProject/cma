use super::super::fixture as resources;
use super::*;
use crate::agent_manager::relay::Relay;
use layer2_core::SubjectId;
pub(super) use resources::context as caller;
pub(super) const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

pub(super) struct ObservedStore {
    pub inner: Arc<InMemorySandboxStore>,
    pub instance: String,
    pub effects: Mutex<Vec<OwnershipToken>>,
}
impl ObservedStore {
    fn mutation(&self, id: &AgentId) {
        let authority = execution_authority().expect("mutation requires owner scope");
        assert!(authority.is_current());
        assert_eq!(authority.token.instance_id, self.instance);
        assert_eq!(&authority.token.agent_id, id);
        assert!(authority.token.epoch > 0);
        self.effects.lock().unwrap().push(authority.token.clone());
    }
}
#[async_trait::async_trait]
impl ResourceAccessStore for ObservedStore {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.inner.resource_access_snapshot(target).await
    }
    async fn organization_sandboxes(
        &self,
        org: &SubjectId,
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(org, query).await
    }
    async fn shared_sandboxes(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(recipient, organizations, query)
            .await
    }
    async fn organization_agents(
        &self,
        org: &SubjectId,
        query: &AgentListQuery,
        page: &store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_agents(org, query, page).await
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for ObservedStore {
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
    async fn share_mutation_receipt(
        &self,
        id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(id).await
    }
    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        let ResourceSelector::Agent { agent_id } = &request.selector else {
            panic!("single-Agent test");
        };
        self.mutation(agent_id);
        self.inner.mutate_share(request).await
    }
}
#[async_trait::async_trait]
impl AgentArtifactStore for ObservedStore {
    async fn publish_artifact(
        &self,
        request: &ArtifactPublication,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        self.mutation(&request.record().binding.agent_id);
        self.inner.publish_artifact(request).await
    }
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        self.inner.agent_artifacts(binding, page).await
    }
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError> {
        self.inner.agent_artifact(binding, id).await
    }
}

pub(super) struct Cluster {
    pub id: AgentId,
    pub inner: Arc<InMemorySandboxStore>,
    pub direct: Arc<ResourceAccessService>,
    pub ownership: Arc<MemoryOwnershipStore>,
    pub writes: Vec<Arc<ObservedStore>>,
    coordinators: Vec<Arc<OwnershipCoordinator>>,
    addresses: Vec<String>,
    servers: Vec<tokio::task::JoinHandle<()>>,
    _updates: Vec<Arc<DurableAgentUpdateRecorder>>,
}
impl Drop for Cluster {
    fn drop(&mut self) {
        for server in &self.servers {
            server.abort();
        }
    }
}
impl Cluster {
    pub async fn new(subject: &str) -> Self {
        let auth = resources::auth(subject);
        let id = AgentId("agt-public-mutation".into());
        let resources = resources::fixture(auth.clone(), id.as_str(), "owner").await;
        let ownership = Arc::new(MemoryOwnershipStore::default());
        let mut value = Self {
            id,
            inner: resources.store.clone(),
            direct: resources.access.clone(),
            ownership,
            writes: Vec::new(),
            coordinators: Vec::new(),
            addresses: Vec::new(),
            servers: Vec::new(),
            _updates: Vec::new(),
        };
        for index in 0..2 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            let coordinator = OwnershipCoordinator::register(
                value.ownership.client(),
                Member {
                    instance_id: format!("mutation-{index}"),
                    protocol: PROTOCOL_VERSION,
                    relay_address: address.clone(),
                    expires_at_ms: 0,
                    draining: false,
                    owned_agents: 0,
                    max_owned_agents: 16,
                },
            )
            .await
            .unwrap();
            let observed = Arc::new(ObservedStore {
                inner: resources.store.clone(),
                instance: coordinator.instance_id.clone(),
                effects: Mutex::default(),
            });
            let access = Arc::new(ResourceAccessService::new(observed.clone(), auth.clone()));
            let updates = DurableAgentUpdateRecorder::new(resources.store.clone());
            let relay = Relay::new(
                coordinator.clone(),
                &updates,
                ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"public-mutation-test"),
            )
            .unwrap();
            let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
            relay
                .install_public_access(PublicAccess::new(
                    auth.clone(),
                    resources.policy.reads.clone(),
                    access.clone(),
                    Arc::new(adapter),
                ))
                .unwrap();
            let app = layer1_public::rest::resource_access_router(access.clone()).merge(
                layer1_public::rest::agent_artifacts_router(AgentArtifactService::new(
                    observed.clone(),
                    access,
                )),
            );
            let app = relay.authenticated_routes(app, auth.clone());
            value.servers.push(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            value.coordinators.push(coordinator);
            value.addresses.push(address);
            value.writes.push(observed);
            value._updates.push(updates);
        }
        value
    }
    pub fn target(&self) -> ResourceTarget {
        ResourceTarget::Agent {
            agent_id: self.id.clone(),
        }
    }
    pub async fn link(&self) -> ResourceShare {
        self.direct
            .create_agent_share_link(
                &caller("owner"),
                None,
                CreateAgentShareLink {
                    agent_id: self.id.clone(),
                    audience: AgentShareAudience::Public,
                    mode: ShareMode::View,
                    token: TOKEN.into(),
                    idempotency_key: "initial-link".into(),
                },
            )
            .await
            .unwrap()
            .share
    }
    pub async fn owner(&self) -> (usize, Arc<Authority>) {
        let selected = candidates(&self.id, &self.ownership.membership().await.unwrap(), None)
            .unwrap()[0]
            .instance_id
            .clone();
        let index = self
            .coordinators
            .iter()
            .position(|coordinator| coordinator.instance_id == selected)
            .unwrap();
        (
            index,
            self.coordinators[index]
                .acquire(&self.id)
                .await
                .unwrap()
                .unwrap(),
        )
    }
    pub async fn request(
        &self,
        ingress: usize,
        method: Method,
        path: &str,
        key: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap()
            .request(method, format!("{}{path}", self.addresses[ingress]))
            .header("Idempotency-Key", key)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&body).unwrap_or_else(|_| json!({"raw":body})),
        )
    }
}
