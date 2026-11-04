use layer3_adapter::{
    hosted::{HostedAgentOpenSettings, HostedBackend},
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, DesktopConnection,
    DesktopConnectionHandle, DesktopFrame, Result, RuntimeHandle,
};
use tokio::sync::mpsc;

pub(super) struct DesktopBackend {
    pub unavailable: bool,
}

#[async_trait::async_trait]
impl HostedBackend for DesktopBackend {
    async fn open_desktop(&self, handle: &RuntimeHandle) -> Result<DesktopConnectionHandle> {
        assert_eq!(handle.as_str(), "runtime-route");
        if self.unavailable {
            return Err(AdapterError::Unavailable(
                "fixture runtime unavailable".into(),
            ));
        }
        let (sender, receiver) = mpsc::channel(4);
        sender
            .send(DesktopFrame::Binary(b"RFB 003.008\n".to_vec()))
            .await
            .unwrap();
        Ok(Box::new(Echo { sender, receiver }))
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        panic!("desktop access must not provision")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("desktop access must not issue runtime commands")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("desktop access must not open an Agent")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("desktop access must not delete a runtime")
    }
}

struct Echo {
    sender: mpsc::Sender<DesktopFrame>,
    receiver: mpsc::Receiver<DesktopFrame>,
}
#[async_trait::async_trait]
impl DesktopConnection for Echo {
    async fn recv(&mut self) -> Result<Option<DesktopFrame>> {
        Ok(self.receiver.recv().await)
    }
    async fn send(&mut self, frame: DesktopFrame) -> Result<()> {
        self.sender.send(frame).await.unwrap();
        Ok(())
    }
}
