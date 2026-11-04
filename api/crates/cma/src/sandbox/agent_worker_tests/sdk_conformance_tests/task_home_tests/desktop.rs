use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend, HostedControlTransport};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{
    AgentLaunch, BrowserObservation, BrowserOperation, BrowserReceipt, BrowserState,
    ConversationChannel, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt,
    HomeVolumeRequest, HomeVolumeStore, RuntimeObservation,
};
use std::collections::BTreeMap;

pub(super) struct CreationSupport {
    pub connections: Arc<dyn layer2_core::sandbox::SandboxCredentialPrerequisite>,
    pub adapter: Option<Arc<dyn SandboxAdapter>>,
}

pub(super) fn image() -> String {
    std::env::var("CMA_DESKTOP_RUNTIME_IMAGE")
        .unwrap_or_else(|_| format!("proof.invalid/cma-runtime@sha256:{}", "d".repeat(64)))
}

pub(super) fn capacity() -> SandboxCapacity {
    SandboxCapacity {
        cpu_millis: 1000,
        memory_mib: 2048,
    }
}

pub(super) fn home_policy() -> HomeVolumePolicy {
    HomeVolumePolicy {
        mode: Default::default(),
        storage_class: "fixture".into(),
        capacity_gib: 64,
        template_name: "desktop".into(),
        template_generation: "1".into(),
    }
}

#[derive(Default)]
struct Homes(Mutex<BTreeMap<String, HomeVolumeAttachment>>);

#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        layer3_adapter::HomeVolumeScope::from_authority(
            "kubernetes-pvc-v1",
            &["ticket16", "local", "proof"],
        )
        .ok()
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
    async fn prepare(
        &self,
        request: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeReceipt> {
        let attachment = HomeVolumeAttachment {
            request: request.clone(),
            receipt: if request.policy.mounts_full_home() {
                super::integrated_tests::home::receipt(request)
            } else {
                HomeVolumeReceipt {
                    native_authority: None,
                    volume_id: request.volume_id.clone(),
                    request_sha256: request.computed_sha256(),
                    storage_reference: "fixture-home".into(),
                    storage_generation: "original-disk".into(),
                }
            },
        };
        Ok(self
            .0
            .lock()
            .unwrap()
            .entry(request.volume_id.clone())
            .or_insert(attachment)
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
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        self.0.lock().unwrap().remove(&request.volume_id);
        Ok(())
    }
}

pub(super) struct RuntimeBoundary {
    backend: InMemoryHostedBackend,
    contexts: Arc<Mutex<BTreeMap<String, String>>>,
    native: Option<super::desktop_native::NativeBoundary>,
    enables: std::sync::atomic::AtomicUsize,
}

impl RuntimeBoundary {
    pub(super) fn verify_recovery(&self) {
        if self.native.is_none() {
            assert!(self.enables.load(std::sync::atomic::Ordering::SeqCst) >= 2);
            assert_eq!(self.contexts.lock().unwrap().len(), 1);
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for RuntimeBoundary {
    async fn reserve_home_request(
        &self,
        _: &ProviderSandboxRequest,
        home: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeRequest> {
        let mut home = home.clone();
        if home.policy.mounts_full_home() {
            home.scope = Homes::default().allocation_scope();
        }
        Ok(home)
    }

    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        let handle = self.backend.provision(request.clone()).await?;
        if let Some(native) = &self.native {
            native.provision(&handle, &request).await?;
        }
        Ok(handle)
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if let Some(native) = &self.native {
            return native.transport.exchange(handle, request).await;
        }
        if let RequestFrame::Browser(request) = request {
            let mut contexts = self.contexts.lock().unwrap();
            if request.operation == BrowserOperation::Enable {
                contexts
                    .entry(request.agent_id.0.clone())
                    .or_insert_with(|| uuid::Uuid::new_v4().to_string());
                if self
                    .enables
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 0
                {
                    return Err(AdapterError::Unavailable(
                        "original browser enabled; receipt lost".into(),
                    ));
                }
            }
            let context_id = contexts.get(&request.agent_id.0).cloned();
            let mut observation = BrowserObservation::idle(if context_id.is_some() {
                BrowserState::Ready
            } else {
                BrowserState::NotEnabled
            });
            observation.context_id = context_id;
            return Ok(ResponseFrame::Browser(BrowserReceipt {
                agent_id: request.agent_id,
                boot_id: request.boot_id,
                observation,
            }));
        }
        let mut response = self.backend.exchange(handle, request).await?;
        if let ResponseFrame::RuntimeStatus(status) = &mut response {
            status.agent_browser = true;
        }
        Ok(response)
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        if let Some(native) = &self.native {
            return native
                .transport
                .open_agent(handle, launch, resume, settings, updates)
                .await;
        }
        let agent = launch.request.agent_id.0.clone();
        let (channel, receipt) = self
            .backend
            .open_agent(handle, launch, resume, settings, updates)
            .await?;
        let contexts = self.contexts.clone();
        let input = ConversationInputSender::spawn(1, move |input| {
            let sender = channel.input.clone();
            let contexts = contexts.clone();
            let agent = agent.clone();
            async move {
                if matches!(
                    &input,
                    ConversationInput::Prompt(_) | ConversationInput::RecoverablePrompt { .. }
                ) {
                    assert!(
                        contexts.lock().unwrap().contains_key(&agent),
                        "desktop must be enabled before the original task"
                    );
                }
                sender.send(input).await
            }
        });
        Ok((
            ConversationChannel {
                input,
                conversation_id: channel.conversation_id,
            },
            receipt,
        ))
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

pub(super) async fn configure(harness: &mut Harness<MongoSandboxStore>) -> Arc<RuntimeBoundary> {
    let native = super::desktop_native::NativeBoundary::from_environment().await;
    let mut spec = CodexSandboxSpec::default();
    if native.is_some() {
        spec.deployment.upstream_base_url = "http://127.0.0.1:16416/v1".into();
        if std::env::var_os("CMA_INTEGRATED_RELEASE_PROOF").is_some() {
            spec.deployment.platform_base_url = "http://127.0.0.1:16416".into();
        }
    }
    let runtime = Arc::new(RuntimeBoundary {
        backend: harness.backend.clone(),
        contexts: Arc::new(Mutex::new(BTreeMap::new())),
        native,
        enables: std::sync::atomic::AtomicUsize::new(0),
    });
    harness.adapter = Arc::new(
        CodexAdapter::new(runtime.clone(), spec).with_home_volumes(Arc::new(Homes::default())),
    );
    let mut config = AgentWorkerConfig::new("desktop-agent-worker", DEPLOYMENT_ID);
    config.retry_delay = Duration::from_secs(1);
    config.open_timeout = Duration::from_secs(60);
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        config,
    );
    runtime
}

pub(super) async fn verify_preflight(harness: &Harness<MongoSandboxStore>, agent: &AgentId) {
    let admitted = harness.stored(agent).await;
    let original = harness
        .store
        .get_sandbox(&admitted.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let policy = layer2_core::sandbox::desktop_creation::DesktopCreationService::new(
        harness.adapter.clone(),
        image(),
        Some(image()),
    )
    .unwrap();
    assert!(policy
        .capability(None, capacity(), Some(&original))
        .await
        .available());
    let mut changed = original.clone();
    changed.status = SandboxStatus::Paused;
    let paused = policy.capability(None, capacity(), Some(&changed)).await;
    assert!(!paused.available());
    assert!(paused.reason.contains("will not restart"));
    changed = original.clone();
    changed.profile.content.memory_mib = 1024;
    assert!(!policy
        .capability(None, capacity(), Some(&changed))
        .await
        .available());
    changed = original.clone();
    changed.runtime.as_mut().unwrap().runtime_version = "old-unverified-runtime".into();
    assert!(!policy
        .capability(None, capacity(), Some(&changed))
        .await
        .available());
    let retained = harness
        .store
        .get_sandbox(&admitted.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.runtime, original.runtime);
    assert_eq!(retained.status, original.status);
}

pub(super) async fn verify_existing_admission(
    service: &Router,
    harness: &Harness<MongoSandboxStore>,
    original_id: &AgentId,
) {
    let original = harness.stored(original_id).await;
    let before = harness
        .store
        .get_sandbox(&original.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let task = "  Use this existing Workspace desktop\nretain this task  ";
    let body = json!({"name":"Existing desktop task", "workspace_id":original.workspace_id,
        "initial_message":task, "desktop":true});
    let (status, receipt) = create(service, &body, "existing-desktop-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let identity = admitted_identity(service, &receipt).await;
    let retained = harness.stored(&identity).await;
    assert_ne!(retained.id, original.id);
    assert_eq!(retained.workspace_id, original.workspace_id);
    assert_eq!(retained.sandbox_id, original.sandbox_id);
    let first = retained.conversation.first_message.as_ref().unwrap();
    assert!(first.desktop);
    assert_eq!(first.text, task);
    let (_, replay) = create(service, &body, "existing-desktop-task").await;
    assert_eq!(receipt["id"], replay["id"]);
    let after = harness
        .store
        .get_sandbox(&original.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.runtime, after.runtime);
    assert_eq!(before.lifecycle, after.lifecycle);
    assert_eq!(before.status, after.status);
}
