use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend, HostedControlTransport};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, HomeVolumeAttachment, HomeVolumeReceipt, HomeVolumeRequest,
    HomeVolumeStore, RuntimeAdoptionRequest, RuntimeObservation,
};

struct RetainedHome(HomeVolumeAttachment);

#[async_trait::async_trait]
impl HomeVolumeStore for RetainedHome {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        self.0.request.scope.clone()
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("a retained reader must not prepare another home")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert_eq!(home, &self.0);
        Ok(())
    }
    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        panic!("a retained reader must not remove the home")
    }
}

struct RetainedProvider {
    native: desktop_native::NativeBoundary,
    observation: RuntimeObservation,
    request: ProviderSandboxRequest,
}

#[async_trait::async_trait]
impl HostedBackend for RetainedProvider {
    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        panic!("a retained reader must not provision another runtime")
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        assert_eq!(handle, &self.observation.handle);
        assert_eq!(request, &self.request);
        Ok(Some(self.observation.clone()))
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        assert_eq!(handle, &self.observation.handle);
        assert!(
            matches!(
                &request,
                RequestFrame::RuntimeStatus(_)
                    | RequestFrame::ListAgentFiles(_)
                    | RequestFrame::ReadAgentFile(_)
            ) || matches!(&request, RequestFrame::Browser(value) if value.operation == layer3_adapter::BrowserOperation::Status)
                || matches!(&request, RequestFrame::BrowserControl(value) if value.command == layer3_adapter::BrowserControlCommand::Status),
            "retained reader attempted a mutation"
        );
        self.native.transport.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("a retained reader must not open another conversation")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("a retained reader must not tear down the runtime")
    }
}

pub(super) async fn adapter(sandbox: &Sandbox, context: &Value) -> Arc<dyn SandboxAdapter> {
    let observation: RuntimeObservation =
        serde_json::from_value(context["observation"].clone()).unwrap();
    let native = desktop_native::NativeBoundary::from_environment()
        .await
        .unwrap();
    native
        .transport
        .bind_runtime(&observation.handle, sandbox.id.as_str())
        .await
        .unwrap();
    let provider = RetainedProvider {
        native,
        observation: observation.clone(),
        request: serde_json::from_value(context["provider_request"].clone()).unwrap(),
    };
    let home = sandbox
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap();
    let mut spec = CodexSandboxSpec::default();
    spec.deployment.upstream_base_url = "http://127.0.0.1:16416/v1".into();
    spec.deployment.platform_base_url = "http://127.0.0.1:16416".into();
    let adapter = Arc::new(
        CodexAdapter::new(Arc::new(provider), spec).with_home_volumes(Arc::new(RetainedHome(home))),
    );
    let request = RuntimeAdoptionRequest::new(
        observation.generation,
        observation.handle.clone(),
        crate::sandbox::runtime_mapping::sandbox_spec(sandbox, DEPLOYMENT_ID).unwrap(),
    )
    .with_expected_image(&sandbox.runtime.as_ref().unwrap().runtime_version);
    assert_eq!(
        adapter.adopt_runtime(&request).await.unwrap(),
        Some(observation)
    );
    adapter
}
