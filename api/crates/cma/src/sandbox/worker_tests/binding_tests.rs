use super::*;
use crate::sandbox::{
    agent_worker::{AgentWorker, AgentWorkerConfig, AgentWorkerDependencies},
    workspace_worker::{WorkspaceWorker, WorkspaceWorkerConfig, WorkspaceWorkerDependencies},
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::profiles::content_digest;
use layer2_core::sandbox::binding::*;
use layer2_core::sandbox::store::{AgentStore as _, WorktreeStore as _};
use layer2_core::sandbox::{
    AgentService, AgentStatus, FixedSandboxCredentialPrerequisite, WorkspaceService, WorktreeStatus,
};
use layer2_core::CoreError;
use serde_json::{json, Value};
use tower::ServiceExt;

struct BindingCustody(Arc<MemoryCredentials>, Arc<InMemorySandboxStore>);
#[async_trait::async_trait]
impl BindingCredentialCustody for BindingCustody {
    async fn retain(
        &self,
        _: &SandboxId,
        _: &SubjectId,
        _: &[ServiceId],
        _: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        unreachable!()
    }
    async fn retain_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        binding.require_credential_admission(key)?;
        let material = crate::sandbox::ManagedSandboxCredentials::binding_material(binding, key)?;
        self.0.restore(material);
        Ok(())
    }
    async fn rotate_binding(
        &self,
        binding: &ProviderBinding,
        key: &ProvidedCredential,
    ) -> Result<ProviderBinding, CoreError> {
        let next = binding.rotate_credential(key.sha256(), layer2_core::now_rfc3339())?;
        let sandbox = self
            .1
            .get_sandbox(&binding.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let mut material = crate::sandbox::ManagedSandboxCredentials::binding_material(&next, key)?;
        material.authority.generation = sandbox.credential.next_generation().unwrap();
        let updated = next.admit_credential_delivery(&sandbox, material.authority.clone())?;
        self.0.restore(material);
        self.1
            .save_sandbox(sandbox.revision, updated)
            .await
            .unwrap();
        self.1
            .save_binding(Some(binding.revision), next.clone())
            .await
            .unwrap();
        Ok(next)
    }
    async fn forget_binding(
        &self,
        binding: &ProviderBinding,
        id: &SandboxId,
    ) -> Result<(), CoreError> {
        binding.require_credential_retirement(id)?;
        self.0
            .forget(id, 1)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}
fn service(h: &Harness, profiles: Arc<InMemoryProfileStore>) -> Arc<BindingService> {
    let prerequisite = Arc::new(FixedSandboxCredentialPrerequisite::SetupRequired);
    BindingService::new(
        h.store.clone(),
        BindingResources {
            sandboxes: SandboxService::new(
                h.store.clone(),
                profiles.clone(),
                h.policy.clone(),
                prerequisite.clone(),
            ),
            workspaces: WorkspaceService::new(h.store.clone(), profiles.clone())
                .with_connections(prerequisite.clone()),
            agents: AgentService::new(
                h.store.clone(),
                profiles,
                h.registry.clone(),
                crate::sandbox::runtime_capabilities(h.adapter.capabilities()),
            )
            .with_connections(prerequisite),
        },
        Arc::new(BindingCustody(h.credentials.clone(), h.store.clone())),
        BindingPolicy {
            provider: AgentProvider::default(),
            supported_model_api: "responses".into(),
            bounds: ProfileCapacityBounds {
                min_cpu_millis: 250,
                max_cpu_millis: 8000,
                min_memory_mib: 512,
                max_memory_mib: 16384,
            },
            sandbox: Arc::new(h.policy.clone()),
        },
    )
}
fn request(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
            "manager",
        )));
    request.extensions_mut().insert(Authorization::new([
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ]));
    request
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    status: StatusCode,
) -> Value {
    let response = app
        .clone()
        .oneshot(request(method, path, body))
        .await
        .unwrap();
    let actual = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(actual, status, "{}", String::from_utf8_lossy(&bytes));
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

#[tokio::test]
async fn binding_rest_to_hosted_runtime_recovers_and_cleans_up_actual_resources() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let profiles = Arc::new(InMemoryProfileStore::default());
    let bindings = service(&h, profiles.clone());
    let app = layer1_public::rest::binding_router(bindings.clone());
    let path = "/api/v1/cmaeg/bindings/binding-runtime";
    let profile = json!({"schema_version":1,"metadata":{"name":"binding-runtime","owner":{"kind":"human","subject":"owner"},"provider":"cma_codex"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"},
            "setup_commands":[{"phase":"before_workspace","command":"prepare-runtime"},{"phase":"after_workspace","command":"prepare-workspace"}]}});
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let body = json!({"schema_version":1,"profile_digest":format!("sha256:{}",content_digest(&profile).unwrap()),"profile":profile,"agent_key":key.expose(),
        "tools":{"native_mcp_url":"https://gateway.example/mcp","nyxid_service_slug":"cmaeg"}});
    let initial = call(&app, "PUT", path, Some(body.clone()), StatusCode::OK).await;
    assert_eq!(initial["state"], "provisioning");
    assert_eq!(
        call(&app, "PUT", path, Some(body.clone()), StatusCode::OK).await,
        initial
    );
    // A fresh control-plane service resumes only the persisted reservation.
    let bindings = service(&h, profiles);
    let app = layer1_public::rest::binding_router(bindings.clone());
    let workspace_worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
        },
        WorkspaceWorkerConfig::new("binding-workspace", "test"),
    );
    let agent_worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: h.store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(crate::sandbox::agent_skills::EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(h.store.clone()),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new("binding-agent", "test"),
    );
    let start = SystemTime::now() + Duration::from_secs(1);
    let sandbox_id = SandboxId(initial["sandbox_id"].as_str().unwrap().into());
    for tick in 0..40 {
        bindings.reconcile(20).await.unwrap();
        let now = start + Duration::from_secs(tick);
        if h.store.get_sandbox(&sandbox_id).await.unwrap().is_some() {
            h.worker.run_one(&sandbox_id, now).await.unwrap();
        }
        workspace_worker.run_due(now).await.unwrap();
        if call(&app, "GET", path, None, StatusCode::OK).await["state"] == "ready" {
            break;
        }
    }
    assert_eq!(
        call(&app, "GET", path, None, StatusCode::OK).await["state"],
        "ready"
    );
    assert!(h.backend.requests().await.iter().any(|(_,request)|matches!(request,RequestFrame::Credential(frame) if frame.value.expose()==key.expose())));
    let conversation_path = format!("{path}/conversations/conversation_runtime");
    let conversation = call(
        &app,
        "PUT",
        &conversation_path,
        Some(json!({})),
        StatusCode::CREATED,
    )
    .await;
    assert_eq!(
        call(
            &app,
            "PUT",
            &conversation_path,
            Some(json!({})),
            StatusCode::OK
        )
        .await,
        conversation
    );
    let agent_id = layer2_core::AgentId(conversation["agent_id"].as_str().unwrap().into());
    for tick in 40..70 {
        bindings.reconcile(20).await.unwrap();
        if h.store.get_agent(&agent_id).await.unwrap().is_some() {
            agent_worker
                .run_one(&agent_id, start + Duration::from_secs(tick))
                .await
                .unwrap();
        }
        if h.store
            .binding_conversation("binding-runtime", 1, "conversation_runtime")
            .await
            .unwrap()
            .unwrap()
            .state
            == BindingConversationState::Ready
        {
            break;
        }
    }
    let agent = h.store.get_agent(&agent_id).await.unwrap().unwrap();
    assert_eq!(agent.status, AgentStatus::Ready);
    assert!(agent.conversation.conversation_id.is_some());
    assert_eq!(
        h.store
            .list_agents(&SubjectId::new("owner"), &PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let before = h.stored(&sandbox_id).await;
    let rotated_key =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let mut rotation_body = body;
    rotation_body["agent_key"] = json!(rotated_key.expose());
    let rotation = call(
        &app,
        "PUT",
        path,
        Some(rotation_body.clone()),
        StatusCode::OK,
    )
    .await;
    assert_eq!(rotation["state"], "provisioning");
    assert_eq!(
        call(&app, "PUT", path, Some(rotation_body), StatusCode::OK).await,
        rotation
    );
    for tick in 70..85 {
        h.worker
            .run_one(&sandbox_id, start + Duration::from_secs(tick))
            .await
            .unwrap();
        bindings.reconcile(20).await.unwrap();
        if call(&app, "GET", path, None, StatusCode::OK).await["state"] == "ready" {
            break;
        }
    }
    let after = h.stored(&sandbox_id).await;
    assert_eq!(after.status, SandboxStatus::Active);
    assert_eq!(after.runtime, before.runtime);
    assert_eq!(after.credential.key_generation, 2);
    assert_eq!(h.store.get_agent(&agent_id).await.unwrap().unwrap(), agent);
    assert_eq!(
        h.store
            .binding("binding-runtime")
            .await
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    assert!(h.backend.requests().await.iter().any(|(_,request)| matches!(request, RequestFrame::Credential(frame) if frame.value.expose() == rotated_key.expose() && frame.custody_generation == Some(2))));
    call(
        &app,
        "DELETE",
        &conversation_path,
        None,
        StatusCode::NO_CONTENT,
    )
    .await;
    for tick in 85..115 {
        bindings.reconcile(20).await.unwrap();
        agent_worker
            .run_one(&agent_id, start + Duration::from_secs(tick))
            .await
            .unwrap();
        if h.store.get_agent(&agent_id).await.unwrap().unwrap().status == AgentStatus::Deleted {
            break;
        }
    }
    assert_eq!(
        h.store.get_agent(&agent_id).await.unwrap().unwrap().status,
        AgentStatus::Deleted
    );
    assert_eq!(
        h.store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    call(&app, "DELETE", path, None, StatusCode::NO_CONTENT).await;
    for tick in 115..160 {
        bindings.reconcile(20).await.unwrap();
        h.worker
            .run_one(&sandbox_id, start + Duration::from_secs(tick))
            .await
            .unwrap();
        if h.store
            .binding("binding-runtime")
            .await
            .unwrap()
            .unwrap()
            .work
            == BindingWork::Idle
        {
            break;
        }
    }
    assert_eq!(h.stored(&sandbox_id).await.status, SandboxStatus::Retired);
    assert!(h
        .credentials
        .custodied(&sandbox_id)
        .await
        .unwrap()
        .is_empty());
    assert!(!h
        .actions
        .snapshot()
        .iter()
        .any(|action| action.starts_with("mint:")));
}
