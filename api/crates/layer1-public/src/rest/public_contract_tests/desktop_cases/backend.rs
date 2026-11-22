use super::ownership::Ownership;
use layer3_adapter::desktop_ownership::DesktopOwnedRuntime;
use layer3_adapter::{
    hosted::{HostedAgentOpenSettings, HostedBackend},
    provider::ProviderSandboxRequest,
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, DesktopConnection,
    DesktopConnectionHandle, DesktopFrame, Result, RuntimeHandle,
};
use layer3_adapter::{DesktopOwnership, DesktopOwnershipChallenge, DesktopOwnershipControl};
use std::sync::Arc;
use tokio::sync::mpsc;

pub(super) struct DesktopBackend {
    pub unavailable: bool,
    pub capacity: bool,
    pub ownership: Arc<Ownership>,
}

#[async_trait::async_trait]
impl HostedBackend for DesktopBackend {
    fn desktop_ownership(&self, _: &RuntimeHandle) -> Option<&dyn DesktopOwnedRuntime> {
        Some(self)
    }
    async fn open_desktop(&self, _: &RuntimeHandle) -> Result<DesktopConnectionHandle> {
        panic!("public desktop must not open an ownerless socket")
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

#[async_trait::async_trait]
impl DesktopOwnedRuntime for DesktopBackend {
    async fn challenge(&self, handle: &RuntimeHandle) -> Result<DesktopOwnershipChallenge> {
        assert_eq!(handle.as_str(), "runtime-route");
        Ok(self.ownership.challenge())
    }
    async fn open(
        &self,
        handle: &RuntimeHandle,
        owner: &DesktopOwnership,
        challenge: &DesktopOwnershipChallenge,
    ) -> Result<DesktopConnectionHandle> {
        assert_eq!(handle.as_str(), "runtime-route");
        if self.capacity {
            let mut failure = super::super::runtime_capacity_cases::capacity_failure();
            failure.kind = layer3_adapter::agent_sandbox::RuntimePermitKind::Stream;
            return Err(failure.into());
        }
        if self.unavailable {
            return Err(AdapterError::Unavailable(
                "fixture runtime unavailable".into(),
            ));
        }
        self.ownership.confirm(owner, challenge);
        self.ownership.state.lock().unwrap().opens += 1;
        let (sender, receiver) = mpsc::channel(4);
        sender
            .send(DesktopFrame::Binary(b"RFB 003.008\n".to_vec()))
            .await
            .unwrap();
        Ok(Box::new(Echo {
            sender,
            receiver,
            ownership: self.ownership.clone(),
            owner: owner.clone(),
        }))
    }
}

struct Echo {
    ownership: Arc<Ownership>,
    owner: DesktopOwnership,
    sender: mpsc::Sender<DesktopFrame>,
    receiver: mpsc::Receiver<DesktopFrame>,
}
#[async_trait::async_trait]
impl DesktopConnection for Echo {
    fn ownership_control(&self) -> Option<Arc<dyn DesktopOwnershipControl>> {
        Some(self.ownership.clone())
    }
    async fn recv(&mut self) -> Result<Option<DesktopFrame>> {
        let frame = self.receiver.recv().await;
        self.ownership.frame(&self.owner)?;
        Ok(frame)
    }
    async fn send(&mut self, frame: DesktopFrame) -> Result<()> {
        self.ownership.frame(&self.owner)?;
        self.sender.send(frame).await.unwrap();
        Ok(())
    }
}
