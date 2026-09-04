use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use super::HostedBackend;
use crate::provider::{ProviderDeleteTarget, ProviderSandboxRequest, SandboxClient};
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{AdapterError, AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle};

const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(180);
const DEFAULT_READY_POLL: Duration = Duration::from_millis(500);

/// Runtime control-link implementation paired with the provider lifecycle.
#[async_trait::async_trait]
pub trait HostedControlTransport: Send + Sync {
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame>;

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: AgentLaunch,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel>;

    async fn retire(&self, handle: &RuntimeHandle) -> crate::Result<()>;
}

/// Production backend: provider lifecycle plus an injected supervisor link.
pub struct ProviderHostedBackend {
    client: SandboxClient,
    transport: Arc<dyn HostedControlTransport>,
    ready_timeout: Duration,
    ready_poll: Duration,
}

impl ProviderHostedBackend {
    pub fn new(client: SandboxClient, transport: Arc<dyn HostedControlTransport>) -> Self {
        Self {
            client,
            transport,
            ready_timeout: DEFAULT_READY_TIMEOUT,
            ready_poll: DEFAULT_READY_POLL,
        }
    }

    #[must_use]
    pub fn with_readiness(mut self, timeout: Duration, poll: Duration) -> Self {
        self.ready_timeout = timeout;
        self.ready_poll = poll;
        self
    }

    pub fn client(&self) -> &SandboxClient {
        &self.client
    }

    async fn await_running(
        &self,
        initial: crate::provider::ProviderSandbox,
    ) -> crate::Result<RuntimeHandle> {
        let handle = RuntimeHandle(initial.id.clone());
        if initial.is_running() {
            return Ok(handle);
        }
        let deadline = tokio::time::Instant::now() + self.ready_timeout;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(AdapterError::Unavailable(format!(
                    "sandbox {} did not start within {} seconds",
                    handle.0,
                    self.ready_timeout.as_secs()
                )));
            }
            tokio::time::sleep(self.ready_poll).await;
            let sandbox = self.client.get(handle.as_str()).await?;
            if sandbox.is_running() {
                return Ok(handle);
            }
        }
    }
}

#[async_trait::async_trait]
impl HostedBackend for ProviderHostedBackend {
    async fn provision(&self, request: ProviderSandboxRequest) -> crate::Result<RuntimeHandle> {
        let sandbox = self.client.create(&request).await?;
        self.await_running(sandbox).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.transport.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: AgentLaunch,
        resume_conversation: Option<String>,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<ConversationChannel> {
        self.transport
            .open_agent(handle, launch, resume_conversation, updates)
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        // Capture immutable provider authority before waiting on local link
        // retirement. A reused provider name can never redirect this delete.
        let target: Option<ProviderDeleteTarget> =
            self.client.capture_delete_target(handle).await?;
        self.transport.retire(handle).await?;
        if let Some(target) = target {
            self.client.delete_captured(target).await?;
        }
        Ok(())
    }
}
