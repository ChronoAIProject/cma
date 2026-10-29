use super::*;
use crate::sandbox::{
    binding_lifecycle_test_support::*,
    worker::{SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies},
    CredentialError, CredentialMaterial, LiveAgentRegistry, SandboxCredentialController,
};
use layer2_core::profiles::{store::InMemoryProfileStore, ProfileCapacityBounds};
use layer2_core::sandbox::binding::{BindingPolicy, BindingResources, BindingService};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::{
    HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore,
    SandboxAdapter,
};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, SystemTime},
};

pub(super) struct Fixture {
    pub client: mongodb::Client,
    pub database: String,
    pub store: Arc<MongoSandboxStore>,
    pub bindings: Arc<BindingService>,
    pub app: axum::Router,
    sandbox_worker: Arc<SandboxWorker>,
    workspace_worker: Arc<WorkspaceWorker>,
    agent_worker: Arc<AgentWorker>,
}
impl Fixture {
    pub async fn new() -> Self {
        let (client, database, store) = super::super::super::super::mongo_tests::fixture().await;
        let store = Arc::new(store);
        let encryption =
            AuthEncryptionKey::from_base64("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").unwrap();
        let custody = Arc::new(
            MongoSandboxCredentialStore::open(&client, &database, "lifecycle", &encryption)
                .await
                .unwrap(),
        );
        let controller = MongoBindingCredentials::new(store.clone(), custody.clone());
        let profiles = Arc::new(InMemoryProfileStore::default());
        let registry = Arc::new(LiveAgentRegistry::default());
        let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
        let adapter: Arc<dyn SandboxAdapter> =
            Arc::new(adapter.with_home_volumes(Arc::new(Homes::default())));
        let policy = SandboxPolicy {
            retention_grace: Duration::from_secs(3600),
            ..Default::default()
        };
        let prerequisite = Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired);
        let sandboxes = SandboxService::new(
            store.clone(),
            profiles.clone(),
            policy.clone(),
            prerequisite.clone(),
        );
        let workspaces = WorkspaceService::new(store.clone(), profiles.clone())
            .with_connections(prerequisite.clone());
        let agents = AgentService::new(
            store.clone(),
            profiles,
            registry.clone(),
            crate::sandbox::runtime_capabilities(adapter.capabilities()),
        )
        .with_connections(prerequisite);
        let bindings = BindingService::new(
            store.clone(),
            BindingResources {
                sandboxes: sandboxes.clone(),
                workspaces: workspaces.clone(),
                agents,
            },
            controller,
            BindingPolicy {
                provider: Default::default(),
                supported_model_api: "responses".into(),
                bounds: ProfileCapacityBounds {
                    min_cpu_millis: 250,
                    max_cpu_millis: 8000,
                    min_memory_mib: 512,
                    max_memory_mib: 16384,
                },
                sandbox: Arc::new(policy.clone()),
            },
        );
        let operations = Arc::new(
            crate::store::operation::MongoOperationStore::open(&client, &database)
                .await
                .unwrap(),
        );
        let app = layer1_public::rest::binding_router_with_native_tools_alias_and_preparation(
            bindings.clone(),
            "https://gateway.example/mcp",
            "https://gateway.example/mcp",
            true,
        )
        .unwrap()
        .merge(layer1_public::rest::hierarchy_router(
            layer1_public::rest::HierarchyState::new(
                sandboxes,
                workspaces.clone(),
                HierarchyReadService::new(store.clone()),
                operations,
            ),
        ));
        let mut workspace_config = WorkspaceWorkerConfig::new("fixture-workspace", "test");
        workspace_config.retry_delay = Duration::ZERO;
        let workspace_worker = WorkspaceWorker::new(
            WorkspaceWorkerDependencies {
                store: store.clone(),
                adapter: adapter.clone(),
            },
            workspace_config,
        );
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let operations = AgentOperationService::new(store.clone(), registry.clone());
        let mut sandbox_config = SandboxWorkerConfig::new(
            "fixture-sandbox",
            "test",
            CodexSandboxSpec::default().deployment.image.uri,
            policy,
        )
        .with_home_policy(Some(HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "empty".into(),
            template_generation: "fixture".into(),
        }));
        sandbox_config.retry_delay = Duration::ZERO;
        let sandbox_worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: store.clone(),
                adapter: adapter.clone(),
                credentials: Arc::new(Credentials(custody)),
                workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                    workspaces,
                    workspace_worker.clone(),
                )),
                agents: Arc::new(DurableLiveAgentStopper::new(
                    store.clone(),
                    registry.clone(),
                )),
                registry: registry.clone(),
                updates: updates.clone(),
                operations: operations.clone(),
            },
            sandbox_config,
        );
        let mut agent_config = AgentWorkerConfig::new("fixture-agent", "test");
        agent_config.retry_delay = Duration::ZERO;
        let agent_worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter,
                skills: Arc::new(EmptyAgentSkillResolver),
                registry,
                updates,
                operations,
            },
            agent_config,
        );
        Self {
            client,
            database,
            store,
            bindings,
            app,
            sandbox_worker,
            workspace_worker,
            agent_worker,
        }
    }

    pub async fn tick(
        &self,
        sandbox: &layer2_core::SandboxId,
        agent: Option<&layer2_core::AgentId>,
    ) {
        self.bindings.reconcile(20).await.unwrap();
        if self.store.get_sandbox(sandbox).await.unwrap().is_some() {
            self.sandbox_worker
                .run_one(sandbox, SystemTime::now())
                .await
                .unwrap();
        }
        self.workspace_worker
            .run_due(SystemTime::now())
            .await
            .unwrap();
        if let Some(agent) = agent {
            if self.store.get_agent(agent).await.unwrap().is_some() {
                self.agent_worker
                    .run_one(agent, SystemTime::now())
                    .await
                    .unwrap();
            }
        }
    }
}

/// External credential/storage IO are deterministic; lifecycle and Mongo
/// transactions are the production services and workers.
struct Credentials(Arc<MongoSandboxCredentialStore>);
#[async_trait::async_trait]
impl SandboxCredentialController for Credentials {
    async fn issue(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        self.material(&sandbox.id, generation)
            .await?
            .ok_or_else(|| CredentialError::Security("fixture binding credential missing".into()))
    }
    async fn material(
        &self,
        id: &layer2_core::SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.0
            .get(id, generation)
            .await
            .map_err(|e| CredentialError::Unavailable(e.to_string()))
    }
    async fn custodied(
        &self,
        id: &layer2_core::SandboxId,
    ) -> Result<Vec<CredentialMaterial>, CredentialError> {
        self.0
            .list(id)
            .await
            .map_err(|e| CredentialError::Unavailable(e.to_string()))
    }
    async fn forget(
        &self,
        id: &layer2_core::SandboxId,
        generation: u64,
    ) -> Result<(), CredentialError> {
        self.0
            .delete(id, generation)
            .await
            .map_err(|e| CredentialError::Unavailable(e.to_string()))
    }
    async fn revoke(&self, sandbox: &Sandbox, _: &str) -> Result<(), CredentialError> {
        assert!(matches!(
            sandbox.credential.source,
            SandboxCredentialSource::Binding { .. }
        ));
        Ok(())
    }
}

#[derive(Default)]
struct Homes(Mutex<BTreeMap<String, HomeVolumeAttachment>>);
#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        let receipt = HomeVolumeReceipt {
            native_authority: None,
            volume_id: request.volume_id.clone(),
            request_sha256: request.computed_sha256(),
            storage_reference: request.volume_id.replace('_', "-"),
            storage_generation: "fixture-generation".into(),
        };
        let mut values = self.0.lock().unwrap();
        Ok(values
            .entry(request.volume_id.clone())
            .or_insert_with(|| HomeVolumeAttachment {
                request: request.clone(),
                receipt,
            })
            .receipt
            .clone())
    }
    async fn verify(&self, attachment: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert_eq!(
            self.0.lock().unwrap().get(&attachment.request.volume_id),
            Some(attachment)
        );
        Ok(())
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        let original = self.0.lock().unwrap().remove(&request.volume_id).unwrap();
        assert_eq!(original.request, *request);
        assert_eq!(Some(&original.receipt), receipt);
        Ok(())
    }
}
