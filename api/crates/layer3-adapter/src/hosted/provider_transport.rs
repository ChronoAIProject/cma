use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::acp::{Bridge, OpenedBridge, MAX_FRAME_BYTES};
use super::checkpoint_store::{CheckpointArchiveStore, CheckpointArchiveStoreFactory};
use super::checkpoint_transport::{restore_request, successful_checkpoint};
use super::provider_endpoint::RuntimeEndpoint;
use super::{checkpoint_transport, control_correlation, HostedAgentOpenSettings};
use crate::provider::SandboxClient;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, AgentSlot, ConversationChannel, ConversationUpdate, RuntimeHandle,
};

const SUPERVISOR_PORT: u16 = 9090;
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);
const CONTROL_DEADLINE: Duration = Duration::from_secs(60 * 60);

/// Concrete authenticated control and ACP transport for provider-hosted runtimes.
pub struct ProviderHostedControlTransport {
    client: RuntimeDiscovery,
    http: reqwest::Client,
    archive_factory: Arc<dyn CheckpointArchiveStoreFactory>,
    state: Arc<Mutex<TransportState>>,
    subscriptions: Option<Arc<dyn super::HostedSubscriptionSource>>,
}

#[derive(Clone)]
enum RuntimeDiscovery {
    Provider(SandboxClient),
    AgentSandbox(crate::agent_sandbox::AgentSandboxClient),
}

#[derive(Default)]
struct TransportState {
    next_link_id: u64,
    runtime_epochs: BTreeMap<RuntimeHandle, u64>,
    slot_epochs: BTreeMap<(RuntimeHandle, AgentSlot), u64>,
    archives: BTreeMap<RuntimeHandle, BoundArchive>,
    links: BTreeMap<(RuntimeHandle, AgentSlot), ActiveLink>,
}

#[derive(Clone)]
struct BoundArchive {
    sandbox_id: String,
    store: Arc<dyn CheckpointArchiveStore>,
}

#[derive(Clone)]
struct ActiveLink {
    id: u64,
    bridge: Arc<Bridge>,
    reader: tokio::task::AbortHandle,
    relay: tokio::task::AbortHandle,
}

impl ActiveLink {
    async fn stop(self) {
        self.bridge.retire().await;
        self.reader.abort();
        self.relay.abort();
    }
}

impl ProviderHostedControlTransport {
    pub fn with_subscriptions(mut self, source: Arc<dyn super::HostedSubscriptionSource>) -> Self {
        self.subscriptions = Some(source);
        self
    }

    async fn archive(&self, handle: &RuntimeHandle) -> crate::Result<BoundArchive> {
        self.state
            .lock()
            .await
            .archives
            .get(handle)
            .cloned()
            .ok_or_else(|| {
                AdapterError::Unavailable(format!(
                    "runtime {} has no sandbox-bound checkpoint store",
                    handle.0
                ))
            })
    }

    async fn control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let url = endpoint.control()?;
        let auth = self.auth_header(endpoint);
        let exchange = async {
            let response = self
                .http
                .post(url)
                .header(auth.0, auth.1)
                .json(request)
                .send()
                .await
                .map_err(|error| {
                    AdapterError::Unavailable(format!("runtime control request failed: {error}"))
                })?;
            super::provider_http::decode_control(response).await
        };
        let response = tokio::time::timeout(CONTROL_DEADLINE, exchange)
            .await
            .map_err(|_| AdapterError::Unavailable("runtime control request timed out".into()))??;
        control_correlation::validate(request, &response)?;
        Ok(response)
    }

    async fn exchange_at(
        &self,
        handle: &RuntimeHandle,
        endpoint: &RuntimeEndpoint,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        if let Some(restore) = restore_request(&request) {
            let archive = self.archive(handle).await?;
            checkpoint_transport::import(
                &self.http,
                endpoint.checkpoint(&restore.object_reference)?,
                self.auth_header(endpoint),
                restore,
                &archive.store,
            )
            .await?;
        }

        let response = self.control_with_hold_recovery(endpoint, &request).await?;

        if let Some(checkpoint) = successful_checkpoint(&response) {
            let archive = self.archive(handle).await?;
            let exported = checkpoint_transport::export(
                &self.http,
                endpoint.checkpoint(&checkpoint.object_reference)?,
                self.auth_header(endpoint),
                checkpoint,
                &archive.store,
            )
            .await;
            if let Err(error) = exported {
                if matches!(request, RequestFrame::CheckpointForTeardown(_))
                    && !error.is_runtime_loss()
                {
                    return Err(self.recover_rejected_hold(endpoint, &request, error).await);
                }
                return Err(error);
            }
        }

        if let (RequestFrame::CloseAgent(request), ResponseFrame::CloseAgent(_)) =
            (&request, &response)
        {
            self.close_link(handle, &request.request.agent_id).await;
        }
        Ok(response)
    }

    async fn control_with_hold_recovery(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        match self.control(endpoint, request).await {
            Err(error)
                if matches!(request, RequestFrame::CheckpointForTeardown(_))
                    && !error.is_runtime_loss() =>
            {
                Err(self.recover_rejected_hold(endpoint, request, error).await)
            }
            result => result,
        }
    }

    async fn recover_rejected_hold(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
        original: AdapterError,
    ) -> AdapterError {
        let RequestFrame::CheckpointForTeardown(held) = request else {
            return original;
        };
        let recovery =
            RequestFrame::ResumeCheckpointWriters(crate::CheckpointWriterResumeRequest {
                writer_resume_token: held.writer_resume_token.clone(),
            });
        match self.control(endpoint, &recovery).await {
            Ok(ResponseFrame::ResumeCheckpointWriters(_)) => original,
            Ok(ResponseFrame::Failure(failure)) => AdapterError::Unavailable(format!(
                "checkpoint export failed ({original}) and writer recovery was refused: {}",
                failure.detail
            )),
            Ok(_) => AdapterError::Unavailable(format!(
                "checkpoint export failed ({original}) and writer recovery was uncorrelated"
            )),
            Err(recovery) => AdapterError::Unavailable(format!(
                "checkpoint export failed ({original}) and writer recovery failed: {recovery}"
            )),
        }
    }

    async fn open_epochs(&self, key: &(RuntimeHandle, AgentSlot)) -> (u64, u64) {
        let state = self.state.lock().await;
        (
            state.runtime_epochs.get(&key.0).copied().unwrap_or(0),
            state.slot_epochs.get(key).copied().unwrap_or(0),
        )
    }

    async fn register_link(
        &self,
        key: (RuntimeHandle, AgentSlot),
        epochs: (u64, u64),
        opened: OpenedBridge,
    ) -> crate::Result<ConversationChannel> {
        let OpenedBridge {
            channel,
            bridge,
            reader,
            relay,
        } = opened;
        let reader_abort = reader.abort_handle();
        let relay_abort = relay.abort_handle();
        let (id, replaced) = {
            let mut state = self.state.lock().await;
            let current = (
                state.runtime_epochs.get(&key.0).copied().unwrap_or(0),
                state.slot_epochs.get(&key).copied().unwrap_or(0),
            );
            if current != epochs {
                drop(state);
                bridge.retire().await;
                reader_abort.abort();
                relay_abort.abort();
                return Err(AdapterError::Unavailable(
                    "agent link was closed or retired while opening".into(),
                ));
            }
            state.next_link_id = state.next_link_id.wrapping_add(1).max(1);
            let id = state.next_link_id;
            let replaced = state.links.insert(
                key.clone(),
                ActiveLink {
                    id,
                    bridge: Arc::clone(&bridge),
                    reader: reader_abort,
                    relay: relay_abort,
                },
            );
            (id, replaced)
        };
        if let Some(replaced) = replaced {
            replaced.stop().await;
        }

        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let _ = reader.await;
            relay.abort();
            let _ = relay.await;
            bridge.retire().await;
            let mut state = state.lock().await;
            if state.links.get(&key).is_some_and(|active| active.id == id) {
                state.links.remove(&key);
            }
        });
        Ok(channel)
    }

    async fn close_link(&self, handle: &RuntimeHandle, agent_id: &AgentSlot) {
        let key = (handle.clone(), agent_id.clone());
        let link = {
            let mut state = self.state.lock().await;
            let epoch = state.slot_epochs.entry(key.clone()).or_default();
            *epoch = epoch.wrapping_add(1);
            state.links.remove(&key)
        };
        if let Some(link) = link {
            link.stop().await;
        }
    }

    async fn connect(
        &self,
        endpoint: &RuntimeEndpoint,
        agent_id: &AgentSlot,
    ) -> crate::Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let mut url = endpoint.conversation(agent_id.as_str())?;
        url.query_pairs_mut().append_pair("outputAck", "1");
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| AdapterError::Rejected("runtime WebSocket URL is invalid".into()))?;
        let (name, value) = self.auth_header(endpoint);
        let value = value
            .parse()
            .map_err(|_| AdapterError::Rejected("provider API key is not a valid header".into()))?;
        request.headers_mut().insert(name, value);
        let config = WebSocketConfig {
            max_message_size: Some(MAX_FRAME_BYTES + 25),
            max_frame_size: Some(MAX_FRAME_BYTES + 25),
            ..WebSocketConfig::default()
        };
        let connected = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (stream, _) = tokio::time::timeout(CONNECT_DEADLINE, connected)
            .await
            .map_err(|_| {
                AdapterError::Unavailable("runtime WebSocket connection timed out".into())
            })?
            .map_err(|error| {
                AdapterError::Unavailable(format!("runtime WebSocket connection failed: {error}"))
            })?;
        Ok(stream)
    }
}

#[async_trait::async_trait]
impl super::HostedControlTransport for ProviderHostedControlTransport {
    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox_id: &str) -> crate::Result<()> {
        if sandbox_id.is_empty()
            || sandbox_id.len() > 256
            || sandbox_id.trim() != sandbox_id
            || sandbox_id.chars().any(char::is_control)
        {
            return Err(AdapterError::Rejected(
                "checkpoint sandbox identity is invalid".into(),
            ));
        }
        let mut state = self.state.lock().await;
        if let Some(existing) = state.archives.get(handle) {
            if existing.sandbox_id != sandbox_id {
                return Err(AdapterError::Rejected(
                    "runtime handle is already bound to another sandbox".into(),
                ));
            }
            return Ok(());
        }
        let store = self.archive_factory.for_sandbox(sandbox_id)?;
        state.archives.insert(
            handle.clone(),
            BoundArchive {
                sandbox_id: sandbox_id.into(),
                store,
            },
        );
        Ok(())
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let endpoint = self.endpoint(handle).await?;
        self.exchange_at(handle, &endpoint, request).await
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        // Require the sandbox binding, but do not transfer the archive again.
        // The caller already published it and checks the pinned identity.
        self.archive(handle).await?;
        let endpoint = self.endpoint(handle).await?;
        self.control_with_hold_recovery(
            &endpoint,
            &RequestFrame::CheckpointForTeardown(request.clone()),
        )
        .await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        ProviderHostedControlTransport::export_agent_history(self, handle, agent).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        mut settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let key = (handle.clone(), launch.request.agent_id.clone());
        let epochs = self.open_epochs(&key).await;
        let endpoint = self.endpoint(handle).await?;
        let response = self
            .exchange_at(handle, &endpoint, RequestFrame::OpenAgent(launch.clone()))
            .await?;
        let mut receipt = match response {
            ResponseFrame::OpenAgent(receipt) => receipt,
            ResponseFrame::Failure(failure) => {
                return Err(super::provider_http::runtime_failure(failure))
            }
            _ => unreachable!("control correlation verifies open response operation"),
        };
        let socket = self.connect(&endpoint, &launch.request.agent_id).await?;
        let subscription = if settings.own_credential {
            match &self.subscriptions {
                Some(source) => Some(super::subscriptions::HostedSubscriptionBinding {
                    source: source.clone(),
                    sandbox_id: self.archive(handle).await?.sandbox_id,
                    runtime: handle.clone(),
                    agent: launch.request.agent_id.clone(),
                }),
                None => None,
            }
        } else {
            None
        };
        let recovered_prompt = settings.recovered_prompt.take();
        let opened = Bridge::open_with_subscription_and_prompt(
            socket,
            launch.request.agent_id.as_str(),
            launch.request.cwd.clone(),
            (settings, subscription),
            resume_conversation,
            recovered_prompt,
            updates,
        )
        .await?;
        let channel = self.register_link(key, epochs, opened).await?;
        receipt.receipt.conversation_id = channel.conversation_id.clone();
        Ok((channel, receipt))
    }

    async fn retire(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        let links = {
            let mut state = self.state.lock().await;
            let epoch = state.runtime_epochs.entry(handle.clone()).or_default();
            *epoch = epoch.wrapping_add(1);
            state.archives.remove(handle);
            state
                .slot_epochs
                .retain(|(runtime, _), _| runtime != handle);
            let keys: Vec<_> = state
                .links
                .keys()
                .filter(|(runtime, _)| runtime == handle)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| state.links.remove(&key))
                .collect::<Vec<_>>()
        };
        for link in links {
            link.stop().await;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "provider_transport_tests.rs"]
mod tests;

#[path = "provider_discovery.rs"]
mod discovery;

#[path = "provider_history.rs"]
mod history;
