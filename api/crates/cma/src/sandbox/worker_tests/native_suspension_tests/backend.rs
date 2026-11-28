//! A provider object keeps its handle while in-memory runtime Pods come and go.
use super::*;
mod recovery;
mod reopen_fixture;
mod retained_policy;
mod retirement;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
pub(super) use retirement::RetirementProbePause;

#[derive(Clone)]
struct Row {
    request: ProviderSandboxRequest,
    pod: Option<RuntimeHandle>,
    observation: RuntimeObservation,
}

#[derive(Clone, Default)]
pub(super) struct NativeBackend {
    pub retirement_records: Arc<super::retirement_case6::records::FixtureRecords>,
    pub retirement_receiver_url: Arc<Mutex<Option<String>>>,
    pub retirement_incarnation: Arc<AtomicUsize>,
    pub retirement_probe_pause: Arc<Mutex<Option<Arc<RetirementProbePause>>>>,
    inner: InMemoryHostedBackend,
    pub pools: Option<Arc<super::super::native_pool_tests::Pools>>,
    row: Arc<tokio::sync::Mutex<Option<Row>>>,
    pub gc_credentials: Arc<Mutex<Option<Arc<MemoryCredentials>>>>,
    pub home_destroyed: Arc<AtomicBool>,
    pub lose_suspend: Arc<AtomicBool>,
    pub lose_resume: Arc<AtomicBool>,
    pub lose_delete: Arc<AtomicBool>,
    pub reject_reopen: Arc<AtomicBool>,
    pub fail_reopen_transiently: Arc<AtomicBool>,
    pub reopen_output: Arc<reopen_fixture::StartupOutput>,
    pub adoption_pause: Arc<Mutex<Option<Arc<RetirementProbePause>>>>,
    pub hold_reopen: Arc<AtomicBool>,
    pub reopen_entered: Arc<Notify>,
    pub reopen_release: Arc<Notify>,
    pub hold_delete: Arc<AtomicBool>,
    pub reject_credential: Arc<AtomicBool>,
    pub fail_credential_transiently: Arc<AtomicBool>,
    pub recovery_plan: Arc<Mutex<serde_json::Value>>,
    pub recovery_code: Arc<Mutex<Option<&'static str>>>,
    pub recovery_response: Arc<Mutex<Option<Vec<u8>>>>,
    pub replace_after_history: Arc<AtomicBool>,
    pub status_override: Arc<Mutex<Option<layer3_adapter::RuntimeStatus>>>,
    pub close_transport:
        Arc<Mutex<Option<Arc<layer3_adapter::hosted::ProviderHostedControlTransport>>>>,
    pub inspection_transport:
        Arc<Mutex<Option<Arc<layer3_adapter::hosted::ProviderHostedControlTransport>>>>,
    pub deny_inventory: Arc<AtomicBool>,
    pub deny_teardown: Arc<AtomicBool>,
    pub hold_finality: Arc<AtomicBool>,
    pub retirement_home: Arc<Mutex<Option<Arc<dyn layer3_adapter::HomeVolumeStore>>>>,
    pub retirement_client:
        Arc<Mutex<Option<Arc<layer3_adapter::agent_sandbox::AgentSandboxClient>>>>,
    pub timeout_inventory: Arc<AtomicBool>,
    pub nonexecuting: Arc<AtomicBool>,
    pub nonexecuting_inspection_pause: Arc<Mutex<Option<Arc<RetirementProbePause>>>>,
    pub proof_revision: Arc<AtomicU64>,
    pub change_before_suspend: Arc<AtomicBool>,
    pub run_before_suspend: Arc<AtomicBool>,
    pub interrupted_workspace: Arc<AtomicBool>,
    pub calls: ActionLog,
    pub adoptions: Arc<AtomicUsize>,
    pub inputs: Arc<Mutex<Vec<layer3_adapter::ConversationInput>>>,
    pub inspected_generations: Arc<Mutex<Vec<u64>>>,
}

impl NativeBackend {
    pub fn new(inner: InMemoryHostedBackend) -> Self {
        Self {
            inner,
            ..Self::default()
        }
    }
    pub async fn snapshot(&self) -> layer3_adapter::hosted::InMemoryRuntimeSnapshot {
        let row = self.row.lock().await;
        self.inner
            .snapshot(row.as_ref().unwrap().pod.as_ref().unwrap())
            .await
            .unwrap()
    }
    pub async fn request(&self) -> ProviderSandboxRequest {
        self.row.lock().await.as_ref().unwrap().request.clone()
    }
    pub async fn lose_object(&self) {
        self.row.lock().await.take();
    }
    pub async fn set_lifecycle(&self, state: &str) {
        self.row
            .lock()
            .await
            .as_mut()
            .unwrap()
            .observation
            .lifecycle
            .state = state.into();
    }
    async fn pod(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<RuntimeHandle> {
        let rows = self.row.lock().await;
        let row = rows.as_ref().ok_or_else(absent)?;
        assert_eq!(row.observation.handle, *handle);
        row.pod.clone().ok_or_else(absent)
    }
}
fn absent() -> layer3_adapter::AdapterError {
    layer3_adapter::AdapterError::Unavailable("fixture Pod is absent".into())
}

#[async_trait::async_trait]
impl HostedBackend for NativeBackend {
    fn retained_startup_policy(
        &self,
    ) -> Option<&dyn layer3_adapter::hosted::HostedRetainedStartupPolicy> {
        Some(self)
    }

    async fn run_recovery_command(
        &self,
        handle: &RuntimeHandle,
        command: &layer3_adapter::hosted::RuntimeRecoveryCommand,
    ) -> layer3_adapter::Result<Vec<u8>> {
        self.run_recovery_fixture(handle, command).await
    }
    async fn suspend_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
        proof: &layer3_adapter::hosted::NonexecutingRuntimeProof,
    ) -> layer3_adapter::Result<()> {
        if self.change_before_suspend.swap(false, Ordering::SeqCst) {
            self.proof_revision.fetch_add(1, Ordering::SeqCst);
        }
        if self.run_before_suspend.swap(false, Ordering::SeqCst) {
            self.nonexecuting.store(false, Ordering::SeqCst);
        }
        self.verify_nonexecuting_runtime(handle, request, home, proof)
            .await?;
        self.suspend(handle, request).await
    }

    async fn inspect_nonexecuting_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<Option<layer3_adapter::hosted::NonexecutingRuntimeProof>> {
        use sha2::{Digest, Sha256};
        let pause = self.nonexecuting_inspection_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        self.inspected_generations
            .lock()
            .unwrap()
            .push(request.metadata["cma/runtime-generation"].parse().unwrap());
        let rows = self.row.lock().await;
        let row = rows.as_ref().ok_or_else(absent)?;
        if !self.nonexecuting.load(Ordering::SeqCst) || row.pod.is_none() {
            return Ok(None);
        }
        if row.observation.handle != *handle || row.request != *request {
            return Err(absent());
        }
        self.calls.push("inspect-nonexecuting");
        Ok(Some(serde_json::from_value(serde_json::json!({
            "version": 1, "handle": handle,
            "request_sha256": format!("{:x}", Sha256::digest(serde_json::to_vec(request).unwrap())),
            "home_identity": home.identity().unwrap(),
            "sandbox_generation": 1, "sandbox_resource_version": "1",
            "pod_name": "original-pod", "pod_uid": "original-pod-uid",
            "pod_resource_version": self.proof_revision.load(Ordering::SeqCst).to_string(),
            "runtime_container_id": "containerd://original", "runtime_restart_count": 691,
            "state_sha256": "a".repeat(64),
        })).unwrap()))
    }

    async fn reserve_home_request(
        &self,
        _: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeRequest,
    ) -> layer3_adapter::Result<layer3_adapter::HomeVolumeRequest> {
        let mut home = home.clone();
        if let Some(pools) = &self.pools {
            home.scope = layer3_adapter::HomeVolumeStore::allocation_scope(pools.as_ref());
        }
        Ok(home)
    }
    fn native_pools(&self) -> Option<&dyn layer3_adapter::agent_sandbox::NativePoolControl> {
        self.pools
            .as_deref()
            .map(|pool| pool as &dyn layer3_adapter::agent_sandbox::NativePoolControl)
    }
    async fn select_native_pool(
        &self,
        _: &ProviderSandboxRequest,
        _: &layer3_adapter::RuntimeImage,
        home: &layer3_adapter::HomeVolumeRequest,
    ) -> layer3_adapter::Result<Option<layer3_adapter::agent_sandbox::NativePoolBinding>> {
        Ok(self
            .pools
            .as_ref()
            .map(|_| super::super::native_pool_tests::Pools::binding(home)))
    }
    async fn upgrade_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        assert!(home.request.policy.is_native_vct());
        assert_ne!(previous.image.uri, next.image.uri);
        self.calls.push("upgrade");
        self.resume(handle, previous, next).await
    }

    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        let mut rows = self.row.lock().await;
        if let Some(row) = rows.as_ref() {
            assert_eq!(row.request, request);
            return Ok(row.observation.handle.clone());
        }
        self.calls.push("create");
        let pod = self.inner.provision(request.clone()).await?;
        let mut observation = self
            .inner
            .inventory()
            .await?
            .into_iter()
            .find(|row| row.handle == pod)
            .unwrap();
        observation.handle = RuntimeHandle::new(if self.pools.is_some() {
            // Match the original native Claim/Sandbox authority returned by Pools.
            "agent-sandbox:warm-member:member-uid"
        } else {
            "agent-sandbox:retained-crd:original-uid"
        });
        let handle = observation.handle.clone();
        *rows = Some(Row {
            request,
            pod: Some(pod),
            observation,
        });
        Ok(handle)
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        if self.timeout_inventory.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(30)).await;
            return Err(layer3_adapter::AdapterError::Unavailable(
                "fixture provider deadline".into(),
            ));
        }
        if self.deny_inventory.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Rejected(
                "fixture provider denied".into(),
            ));
        }
        Ok(self
            .row
            .lock()
            .await
            .iter()
            .map(|row| row.observation.clone())
            .collect())
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.adoptions.fetch_add(1, Ordering::SeqCst);
        let pause = self.adoption_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
        let rows = self.row.lock().await;
        let Some(row) = rows.as_ref() else {
            return Ok(None);
        };
        assert_eq!(row.observation.handle, *handle);
        assert_eq!(row.request, *request);
        Ok(Some(row.observation.clone()))
    }
    async fn reclaim(&self, _: &RuntimeObservation) -> layer3_adapter::Result<()> {
        panic!("reconciliation must retain the suspended CRD");
    }
    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<()> {
        let mut rows = self.row.lock().await;
        let row = rows.as_mut().ok_or_else(absent)?;
        assert_eq!(row.observation.handle, *handle);
        assert_eq!(row.request, *request);
        if let Some(pod) = row.pod.take() {
            self.calls.push("suspend");
            self.inner.teardown(&pod).await?;
            row.observation.lifecycle.state = "suspended".into();
        }
        if self.lose_suspend.swap(false, Ordering::SeqCst) {
            return Err(absent());
        }
        Ok(())
    }
    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        _for_retirement: bool,
    ) -> layer3_adapter::Result<()> {
        let rows = self.row.lock().await;
        let row = rows.as_ref().ok_or_else(absent)?;
        assert_eq!(row.observation.handle, *handle);
        assert_eq!(row.request, *request);
        if row.pod.is_some() {
            return Err(absent());
        }
        self.calls.push("verify-suspended");
        Ok(())
    }
    async fn resume(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<()> {
        let mut rows = self.row.lock().await;
        let row = rows.as_mut().ok_or_else(absent)?;
        assert_eq!(row.observation.handle, *handle);
        if row.request == *next && row.pod.is_some() {
            self.calls.push("resume-replay");
            return Ok(());
        }
        assert_eq!(row.request, *previous);
        assert!(row.pod.is_none());
        self.calls.push("resume");
        let pod = self.inner.provision(next.clone()).await?;
        let mut observation = self
            .inner
            .inventory()
            .await?
            .into_iter()
            .find(|row| row.handle == pod)
            .unwrap();
        observation.handle = handle.clone();
        *row = Row {
            request: next.clone(),
            pod: Some(pod),
            observation,
        };
        if self.lose_resume.swap(false, Ordering::SeqCst) {
            return Err(absent());
        }
        Ok(())
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if let Some((transport, target)) = self.retirement_exchange(handle, &request) {
            use layer3_adapter::hosted::HostedControlTransport;
            return transport.exchange(&target, request).await;
        }
        if matches!(request, RequestFrame::PrepareWorkspace(_))
            && self.interrupted_workspace.load(Ordering::SeqCst)
        {
            self.calls.push("workspace-indeterminate");
            return Ok(ResponseFrame::Failure(layer3_adapter::wire::WireFailure {
                operation: WireOperation::PrepareWorkspace,
                code: "workspace_prepare_outcome_unknown".into(),
                detail: "staged preparation has no completed identity metadata".into(),
                retryable: false,
            }));
        }
        if matches!(request, RequestFrame::Credential(_)) {
            self.credential_delivery_fault()?;
        }
        let mut response = self
            .inner
            .exchange(&self.pod(handle).await?, request)
            .await?;
        if let ResponseFrame::RuntimeStatus(status) = &mut response {
            if let Some(replacement) = self.status_override.lock().unwrap().as_ref() {
                *status = replacement.clone();
            }
        }
        Ok(response)
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        if let Some(native) = &resume {
            self.before_reopen(native, &updates).await?;
        }
        let (mut channel, receipt) = self
            .inner
            .open_agent(&self.pod(handle).await?, launch, resume, settings, updates)
            .await?;
        let original = channel.input;
        let inputs = self.inputs.clone();
        channel.input = layer3_adapter::ConversationInputSender::spawn(8, move |input| {
            inputs.lock().unwrap().push(input.clone());
            let original = original.clone();
            async move { original.send(input).await }
        });
        Ok((channel, receipt))
    }
    async fn challenge_retirement_receiver(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        _: &layer3_adapter::HomeVolumeAttachment,
        receiver: &layer3_adapter::receiver_failure::RetirementReceiver,
    ) -> layer3_adapter::Result<bool> {
        self.challenge_retirement_fixture(handle, request, receiver)
            .await
    }
    async fn retire_verified(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &layer3_adapter::HomeVolumeAttachment,
        pod: Option<&layer3_adapter::RetirementPod>,
    ) -> layer3_adapter::Result<()> {
        self.retire_verified_fixture(handle, request, home, pod)
            .await
    }
    async fn verify_retired(
        &self,
        handle: &RuntimeHandle,
        home: &layer3_adapter::HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        self.verify_retired_fixture(handle, home).await
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.calls.push("delete-retained");
        let client = self.retirement_client.lock().unwrap().clone();
        if let Some(client) = client {
            client.delete(handle).await?;
        }
        if self.deny_teardown.load(Ordering::SeqCst) {
            return Err(absent());
        }
        if self.hold_delete.load(Ordering::SeqCst) {
            return Err(absent());
        }
        self.teardown(handle).await?;
        if self.lose_delete.swap(false, Ordering::SeqCst) {
            return Err(absent());
        }
        Ok(())
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        let mut rows = self.row.lock().await;
        if let Some(row) = rows.take() {
            self.calls.push("delete-effective");
            if let Some(credentials) = self.gc_credentials.lock().unwrap().as_ref() {
                assert!(
                    credentials.live_keys.lock().unwrap().is_empty(),
                    "physical GC with a live key"
                );
                assert!(
                    credentials.materials.lock().unwrap().is_empty(),
                    "physical GC with custody debt"
                );
                // Provider GC occurs during deletion, independently of release_home and its ACK.
                self.home_destroyed.store(true, Ordering::SeqCst);
            }
            assert_eq!(row.observation.handle, *handle);
            if let Some(pod) = row.pod {
                self.inner.teardown(&pod).await?
            }
        }
        Ok(())
    }
}
