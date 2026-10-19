//! A provider object keeps its handle while in-memory runtime Pods come and go.
use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};

#[derive(Clone)]
struct Row {
    request: ProviderSandboxRequest,
    pod: Option<RuntimeHandle>,
    observation: RuntimeObservation,
}

#[derive(Clone, Default)]
pub(super) struct NativeBackend {
    inner: InMemoryHostedBackend,
    pub pools: Option<Arc<super::super::native_pool_tests::Pools>>,
    row: Arc<tokio::sync::Mutex<Option<Row>>>,
    pub lose_suspend: Arc<AtomicBool>,
    pub lose_resume: Arc<AtomicBool>,
    pub lose_delete: Arc<AtomicBool>,
    pub reject_credential: Arc<AtomicBool>,
    pub status_override: Arc<Mutex<Option<layer3_adapter::RuntimeStatus>>>,
    pub calls: ActionLog,
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
    pub async fn restart_process(&self) {
        let rows = self.row.lock().await;
        let pod = rows.as_ref().unwrap().pod.as_ref().unwrap();
        self.inner.restart_persistent_process(pod).await.unwrap();
    }
    pub async fn lose_object(&self) {
        self.row.lock().await.take();
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
        observation.handle = RuntimeHandle::new("agent-sandbox:retained-crd:original-uid");
        let handle = observation.handle.clone();
        *rows = Some(Row {
            request,
            pod: Some(pod),
            observation,
        });
        Ok(handle)
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
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
    ) -> layer3_adapter::Result<()> {
        let rows = self.row.lock().await;
        let row = rows.as_ref().ok_or_else(absent)?;
        assert_eq!(row.observation.handle, *handle);
        assert_eq!(row.request, *request);
        assert!(row.pod.is_none());
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
        if matches!(request, RequestFrame::Credential(_))
            && self.reject_credential.load(Ordering::SeqCst)
        {
            return Err(layer3_adapter::AdapterError::Rejected(
                "fixture credential delivery rejected".into(),
            ));
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
        self.inner
            .open_agent(&self.pod(handle).await?, launch, resume, settings, updates)
            .await
    }
    async fn teardown_retained(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.calls.push("delete-retained");
        self.teardown(handle).await?;
        if self.lose_delete.swap(false, Ordering::SeqCst) {
            return Err(absent());
        }
        Ok(())
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        let mut rows = self.row.lock().await;
        if let Some(row) = rows.take() {
            assert_eq!(row.observation.handle, *handle);
            if let Some(pod) = row.pod {
                self.inner.teardown(&pod).await?
            }
        }
        Ok(())
    }
}
