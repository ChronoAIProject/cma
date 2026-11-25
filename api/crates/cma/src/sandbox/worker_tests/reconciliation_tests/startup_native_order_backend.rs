use super::*;
use layer3_adapter::hosted::{
    AgentSandboxBackend, HostedAgentOpenSettings, HostedBackend, HostedControlTransport,
};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, ConversationUpdate, HomeVolumeAttachment, HomeVolumeReceipt,
    HomeVolumeRequest, HomeVolumeStore, RuntimeObservation, RuntimeStatus,
};

pub(super) struct Transport(pub Arc<Control>);

#[async_trait::async_trait]
impl HostedControlTransport for Transport {
    async fn bind_runtime(&self, handle: &RuntimeHandle, _: &str) -> layer3_adapter::Result<()> {
        assert!(!self.0.api.lock().unwrap().assignments.is_empty());
        self.0.record("NATIVE_BIND", handle.as_str());
        Ok(())
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let RequestFrame::RuntimeStatus(request) = request else {
            panic!("only status is expected during startup")
        };
        self.0.record("STATUS_ENTER", handle.as_str());
        self.0.status_entered.notify_one();
        self.0.status_gate.acquire().await.unwrap().forget();
        self.0.record("STATUS_RETURN", handle.as_str());
        Ok(ResponseFrame::RuntimeStatus(RuntimeStatus {
            sandbox_id: request.sandbox_id,
            generation: request.generation,
            home_identity: request.home_identity,
            boot_id: "a".repeat(32),
            installation_ready: !self.0.refuse_status,
            credential_ready: !self.0.refuse_status,
            restore_pending: false,
            agent_processes: u16::from(!self.0.refuse_status),
            agent_filesystem: false,
            agent_attachment_upload: false,
            agent_skill_install: true,
            agent_skill_upgrade: false,
            agent_model_catalog: true,
            agent_browser: false,
            agent_resource_preparation: false,
            agent_process_restart: false,
            exact_github_credential_route: false,
            github_repository_scope: false,
        }))
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("startup must not open agents")
    }
    async fn retire(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("startup must not retire a retained runtime")
    }
}

pub(super) struct Backend {
    pub native: AgentSandboxBackend,
    pub other: InMemoryHostedBackend,
    pub control: Arc<Control>,
}

struct Active<'a>(&'a Control, bool);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.other_active.fetch_sub(1, Ordering::SeqCst);
        if !self.1 {
            self.0.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for Backend {
    fn startup_binding_headroom(&self, handles: &[RuntimeHandle]) -> Option<usize> {
        Some(
            self.native
                .startup_binding_headroom(handles)?
                .min(self.other.startup_binding_headroom(handles)?),
        )
    }
    fn native_pools(&self) -> Option<&dyn layer3_adapter::agent_sandbox::NativePoolControl> {
        self.native.native_pools()
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        panic!("startup must not provision runtimes")
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        let mut rows = self.native.inventory().await?;
        rows.extend(self.other.inventory().await?);
        Ok(rows)
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        if handle.as_str().starts_with("agent-sandbox:") {
            return self.native.adopt(handle, request).await;
        }
        self.control.other_started.fetch_add(1, Ordering::SeqCst);
        self.control.other_active.fetch_add(1, Ordering::SeqCst);
        self.control.other_entered.notify_one();
        let mut active = Active(&self.control, false);
        self.control.other_gate.acquire().await.unwrap().forget();
        let result = self.other.adopt(handle, request).await;
        self.control.other_settled.fetch_add(1, Ordering::SeqCst);
        active.1 = true;
        result
    }
    async fn reclaim(&self, _: &RuntimeObservation) -> layer3_adapter::Result<()> {
        panic!("all startup runtimes have retained owners")
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if handle.as_str().starts_with("agent-sandbox:") {
            self.native.exchange(handle, request).await
        } else {
            self.other.exchange(handle, request).await
        }
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("startup must not open agents")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("startup must not tear down retained runtimes")
    }
}

pub(super) struct Homes {
    pub inner: Arc<dyn HomeVolumeStore>,
    pub control: Arc<Control>,
}
#[async_trait::async_trait]
impl HomeVolumeStore for Homes {
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        self.control.assert_siblings_settled();
        assert!(self.control.cleanup.load(Ordering::SeqCst) > 0);
        self.control.record("HOME_PHASE", "");
        self.inner.inventory().await
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        if self
            .control
            .hold_next_home_check
            .swap(false, Ordering::SeqCst)
        {
            self.control.home_check_entered.notify_one();
            self.control
                .home_check_gate
                .acquire()
                .await
                .unwrap()
                .forget();
        }
        self.inner.verify(home).await
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("startup must not allocate homes")
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("startup must not remove retained homes")
    }
}
