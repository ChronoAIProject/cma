use super::*;
use l3::hosted::{HostedAgentOpenSettings, HostedBackend, ProviderHostedControlTransport};
use l3::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};

pub(super) struct FileBackend(pub Arc<ProviderHostedControlTransport>);

#[async_trait::async_trait]
impl HostedBackend for FileBackend {
    async fn provision(
        &self,
        _: l3::provider::ProviderSandboxRequest,
    ) -> l3::Result<l3::RuntimeHandle> {
        Ok(l3::RuntimeHandle::new(HANDLE))
    }

    async fn exchange(
        &self,
        handle: &l3::RuntimeHandle,
        request: RequestFrame,
    ) -> l3::Result<ResponseFrame> {
        self.0.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        _: &l3::RuntimeHandle,
        _: FencedRequest<l3::AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<l3::ConversationUpdate>,
    ) -> l3::Result<(l3::ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("file reads do not open an Agent")
    }

    async fn teardown(&self, _: &l3::RuntimeHandle) -> l3::Result<()> {
        panic!("file reads do not tear down a runtime")
    }
}

pub(super) struct Home(pub l3::HomeVolumeAttachment);

#[async_trait::async_trait]
impl l3::HomeVolumeStore for Home {
    async fn inventory(&self) -> l3::Result<Vec<l3::HomeVolumeAttachment>> {
        Ok(vec![self.0.clone()])
    }

    async fn prepare(&self, _: &l3::HomeVolumeRequest) -> l3::Result<l3::HomeVolumeReceipt> {
        panic!("the fixture volume is already allocated")
    }

    async fn verify(&self, home: &l3::HomeVolumeAttachment) -> l3::Result<()> {
        assert_eq!(home, &self.0);
        Ok(())
    }

    async fn remove(
        &self,
        _: &l3::HomeVolumeRequest,
        _: Option<&l3::HomeVolumeReceipt>,
    ) -> l3::Result<()> {
        panic!("file reads do not remove a volume")
    }
}
