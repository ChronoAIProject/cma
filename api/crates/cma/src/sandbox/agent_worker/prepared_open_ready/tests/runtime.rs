use super::*;
use layer2_core::sandbox::store::{SandboxStore, WorktreeStore};
use layer3_adapter::{
    AgentHomeRequest, AgentOpenRequest, CredentialFrame, HomeVolumeAttachment, HomeVolumeReceipt,
    HomeVolumeRequest, HomeVolumeStore, InstallRequest, RuntimeCredential, RuntimeProvisionRequest,
    SandboxAdapter, WorkspaceRequest, WorkspaceSource, WorktreeRequest,
};
use mongodb::bson::{doc, Document};

struct Storage(HomeVolumeAttachment);
#[async_trait::async_trait]
impl HomeVolumeStore for Storage {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        self.0.request.scope.clone()
    }
    fn accepts_scope(&self, scope: &layer3_adapter::HomeVolumeScope) -> bool {
        self.0.request.scope.as_ref() == Some(scope)
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("already prepared")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        if home == &self.0 {
            Ok(())
        } else {
            Err(layer3_adapter::AdapterError::Rejected(
                "home changed".into(),
            ))
        }
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("no teardown")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_uses_one_fresh_inspection_and_real_handshake_through_connection() {
    let mut fixture = Fixture::new().await;
    fixture
        .worker
        .dependencies
        .registry
        .rollback(&fixture.attachment);
    let mut config = layer3_adapter::codex::CodexSandboxSpec::default();
    config.deployment.image.uri = fixture
        .context
        .sandbox
        .runtime
        .as_ref()
        .unwrap()
        .runtime_version
        .clone();
    let (adapter, backend) = layer3_adapter::codex::CodexAdapter::in_memory(config);
    let home = fixture
        .context
        .sandbox
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    let adapter = Arc::new(adapter.with_home_volumes(Arc::new(Storage(home))));
    let spec =
        crate::sandbox::runtime_mapping::sandbox_spec(&fixture.context.sandbox, "test").unwrap();
    let runtime = adapter
        .provision_runtime(&RuntimeProvisionRequest::new(1, spec))
        .await
        .unwrap();
    adapter
        .run_install(
            &runtime,
            &InstallRequest::new("prepared-open-install", vec![]),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(
            &runtime,
            CredentialFrame {
                operation_id: "prepared-open-credential".into(),
                value: RuntimeCredential::new("opaque-sandbox-key"),
                custody_generation: Some(3),
                github_route: None,
            },
        )
        .await
        .unwrap();
    let status = adapter.runtime_status(&runtime).await.unwrap();
    let workspace = adapter
        .prepare_workspace(
            &runtime,
            &WorkspaceRequest::new(
                format!("workspace:{}:prepare:v1", fixture.context.workspace.id),
                fixture.context.workspace.id.to_string(),
                "workspace",
                WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();
    let add = WorktreeRequest::new(
        "worktree:workspace:wt-1:add:v1",
        "workspace",
        "wt-1",
        workspace.resolved_commit.clone(),
    );
    adapter.add_worktree(&runtime, 1, &add).await.unwrap();
    let options = crate::sandbox::runtime_options(
        &fixture.agent,
        &fixture.context.sandbox,
        &fixture.context.worktree,
    );
    let home = AgentHomeRequest::new(
        format!("agent:{}:home:create:v1", fixture.agent.id),
        options.clone(),
        String::new(),
        vec![],
    );
    adapter
        .materialize_agent_home(&runtime, 1, &home)
        .await
        .unwrap();
    let open = AgentOpenRequest::new(format!("agent:{}:open:v1", fixture.agent.id), options, None);
    let (updates, _receiver) = tokio::sync::mpsc::channel(256);
    let channel = adapter
        .open_agent(&runtime, 1, &open, updates)
        .await
        .unwrap();
    let conversation = channel.conversation_id.unwrap();

    let previous = fixture.context.sandbox.revision;
    let binding = fixture.context.sandbox.runtime.as_mut().unwrap();
    binding.runtime_id = runtime.as_str().into();
    binding.boot_id = Some(status.boot_id);
    fixture.context.sandbox.revision += 1;
    fixture
        .store
        .save_sandbox(previous, fixture.context.sandbox.clone())
        .await
        .unwrap();
    let previous = fixture.context.workspace.revision;
    fixture.context.workspace.resolved_commit = Some(workspace.resolved_commit.clone());
    fixture.context.workspace.revision += 1;
    fixture
        .store
        .save_workspace(previous, fixture.context.workspace.clone())
        .await
        .unwrap();
    let previous = fixture.context.worktree.revision;
    fixture.context.worktree.base_commit = workspace.resolved_commit.clone();
    fixture.context.worktree.revision += 1;
    fixture
        .store
        .save_worktree(previous, fixture.context.worktree.clone())
        .await
        .unwrap();
    fixture.client.database(&fixture.name).collection::<Document>("native_prepared_sandboxes")
        .update_one(doc! {"_id":fixture.context.sandbox.id.as_str()},doc! {"$set":{
            "record.payload.runtime":mongodb::bson::serialize_to_bson(&fixture.context.sandbox.runtime).unwrap(),
            "record.payload.workspaces.0.resolved_commit":&workspace.resolved_commit,
            "record.payload.workspaces.0.agents.0.conversation_id":&conversation,
        }}).await.unwrap();
    let previous = fixture.agent.revision;
    fixture.agent.work.warm_conversation_id = Some(conversation.clone());
    fixture.agent.work.receipts[0].request_sha256 = add.request_sha256;
    fixture.agent.work.lease.as_mut().unwrap().fence = 2;
    fixture.agent.work.next_fence = 3;
    fixture.agent.revision += 1;
    fixture
        .store
        .save_agent(previous, fixture.agent.clone())
        .await
        .unwrap();
    let registry = fixture.worker.dependencies.registry.clone();
    fixture.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: fixture.store.clone(),
            adapter,
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(fixture.store.clone()),
            operations: AgentOperationService::new(fixture.store.clone(), registry),
        },
        AgentWorkerConfig::new("open-worker", "test"),
    );
    let now = SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(
            layer2_core::seconds_from_rfc3339("2026-09-05T00:00:00Z").unwrap(),
        );
    let before = backend.requests().await.len();
    fixture
        .worker
        .open_conversation(fixture.agent.clone(), now)
        .await
        .unwrap();
    let requests = backend.requests().await;
    let ready = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.status, AgentStatus::Ready, "{:?}", ready.failure);
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|(_, r)| matches!(r, layer3_adapter::wire::RequestFrame::RuntimeStatus(_)))
            .count(),
        1
    );
    assert_eq!(
        requests[before..]
            .iter()
            .filter(|(_, r)| matches!(r, layer3_adapter::wire::RequestFrame::OpenAgent(_)))
            .count(),
        1
    );
    assert_eq!(ready.work.step, AgentWorkStep::None);
    assert_eq!(
        ready.conversation.conversation_id.as_deref(),
        Some(conversation.as_str())
    );
    assert!(fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&LiveAgentTarget {
            agent_id: ready.id,
            conversation_id: conversation,
            runtime_generation: 1
        }));
    fixture.cleanup().await;
}
