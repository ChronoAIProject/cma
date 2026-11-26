use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::acp::{Bridge, OpenedBridge, Socket, MAX_FRAME_BYTES};
use super::checkpoint_store::{CheckpointArchiveStore, CheckpointArchiveStoreFactory};
use super::checkpoint_transport::{restore_request, successful_checkpoint};
use super::provider_endpoint::{NativeRuntimeIdentity, RuntimeEndpoint};
use super::{checkpoint_transport, control_correlation, HostedAgentOpenSettings};
use crate::agent_sandbox::{RuntimeCapacityAuthority, RuntimeStreamKind, RuntimeWorkloadClass};
use crate::provider::SandboxClient;
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AdapterError, AgentLaunch, AgentSlot, ConversationChannel, ConversationUpdate, RuntimeHandle,
};

const SUPERVISOR_PORT: u16 = 9090;
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);
const CONTROL_DEADLINE: Duration = Duration::from_secs(60 * 60);

/// Runtime data path, independent of Kubernetes resource allocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NativeRuntimeTransport {
    #[default]
    Direct,
    KubernetesPortforward,
    KubernetesExec,
}

/// Concrete authenticated control and ACP transport for provider-hosted runtimes.
#[derive(Clone)]
pub struct ProviderHostedControlTransport {
    output_json_budget: bool,
    client: RuntimeDiscovery,
    owner_router: Option<Arc<crate::owner_control::OwnerControlRoute>>,
    http: reqwest::Client,
    desktop_http: reqwest::Client,
    archive_factory: Option<Arc<dyn CheckpointArchiveStoreFactory>>,
    state: Arc<Mutex<TransportState>>,
    subscriptions: Option<Arc<dyn super::HostedSubscriptionSource>>,
    open_reply_loss: Option<(String, Arc<dyn super::HostedOpenReplyLoss>)>,
    native_transport: NativeRuntimeTransport,
    legacy_receiver_image: Option<LegacyReceiverImage>,
    stale_turn_image: Option<String>,
    runtime_capacity: Arc<RuntimeCapacityAuthority>,
    #[cfg(test)]
    native_http_budget: Arc<tokio::sync::Semaphore>,
    coordinated_ingress: bool,
    retained_json: bool,
    bounded_raw_output: bool,
    bounded_permission_state: bool,
    bounded_update_relay: bool,
    shared_runtime_output: bool,
    native_close_agent_lane: bool,
}

#[derive(Clone)]
enum RuntimeDiscovery {
    Provider(Arc<SandboxClient>),
    AgentSandbox(Arc<crate::agent_sandbox::AgentSandboxClient>),
}

#[derive(Default)]
struct TransportState {
    native_http_telemetry: Option<Arc<native_pool::telemetry::Pool>>,
    desktop_challenges: BTreeMap<String, desktop::Pending>,
    next_link_id: u64,
    runtime_epochs: BTreeMap<RuntimeHandle, u64>,
    slot_epochs: BTreeMap<(RuntimeHandle, AgentSlot), u64>,
    bindings: BTreeMap<RuntimeHandle, BoundRuntime>,
    links: BTreeMap<(RuntimeHandle, AgentSlot), ActiveLink>,
    native_http:
        BTreeMap<(RuntimeHandle, native_pool::NativeHttpLane), Arc<native_pool::NativeHttpSlot>>,
}

#[derive(Clone)]
struct BoundRuntime {
    sandbox_id: String,
    request_sha256: Option<String>,
    store: Option<Arc<dyn CheckpointArchiveStore>>,
}

#[derive(Clone)]
struct ActiveLink {
    id: u64,
    bridge: Arc<Bridge>,
    reader: tokio::task::AbortHandle,
    relay: tokio::task::AbortHandle,
    _ownership: Option<Arc<ownership::ReceiverLease>>,
}

impl ActiveLink {
    async fn stop(self) {
        self.bridge.retire().await;
        self.reader.abort();
        self.relay.abort();
    }
}

impl ProviderHostedControlTransport {
    pub fn with_owner_router(
        mut self,
        router: Arc<crate::owner_control::OwnerControlRoute>,
    ) -> Self {
        self.owner_router = Some(router);
        self
    }

    fn pin_output_policy(&self, settings: &mut HostedAgentOpenSettings) {
        settings.coordinated_ingress = self.coordinated_ingress;
        settings.retained_json = self.retained_json;
        settings.bounded_raw_output = self.bounded_raw_output;
        settings.bounded_permission_state = self.bounded_permission_state;
        settings.output_json_budget = self.output_json_budget;
        settings.shared_runtime_output = self.shared_runtime_output;
    }

    pub fn with_coordinated_ingress(mut self, enabled: bool) -> Self {
        self.coordinated_ingress = enabled;
        self
    }

    pub fn with_retained_json(mut self, enabled: bool) -> Self {
        self.retained_json = enabled;
        self
    }

    pub fn with_bounded_raw_output(mut self, enabled: bool) -> Self {
        self.bounded_raw_output = enabled;
        self
    }

    pub fn with_bounded_permission_state(mut self, enabled: bool) -> Self {
        self.bounded_permission_state = enabled;
        self
    }

    pub fn with_bounded_update_relay(mut self, enabled: bool) -> Self {
        self.bounded_update_relay = enabled;
        self
    }

    pub fn with_output_json_budget(mut self, enabled: bool) -> Self {
        self.output_json_budget = enabled;
        self
    }

    pub fn with_shared_runtime_output(mut self, enabled: bool) -> Self {
        self.shared_runtime_output = enabled;
        self
    }

    pub fn with_subscriptions(mut self, source: Arc<dyn super::HostedSubscriptionSource>) -> Self {
        self.subscriptions = Some(source);
        self
    }

    async fn exchange_at(
        &self,
        handle: &RuntimeHandle,
        endpoint: &RuntimeEndpoint,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.require_supported_operation(&request)?;
        if let Some(restore) = restore_request(&request) {
            let archive = self.archive(handle).await?;
            checkpoint_transport::import(
                &self.http,
                endpoint.checkpoint(&restore.object_reference)?,
                self.auth_header(endpoint),
                restore,
                &archive,
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
                &archive,
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
        ownership: Option<ownership::ReceiverLease>,
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
                    _ownership: ownership.map(Arc::new),
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
    ) -> crate::Result<Socket> {
        let url = endpoint.conversation(agent_id.as_str())?;
        self.connect_url(
            endpoint,
            agent_id,
            url,
            None,
            RuntimeWorkloadClass::Ordinary,
        )
        .await
    }

    async fn connect_with_prepared_stream(
        &self,
        endpoint: &RuntimeEndpoint,
        agent_id: &AgentSlot,
        prepared: Option<crate::agent_sandbox::RuntimeStream>,
    ) -> crate::Result<Socket> {
        let url = endpoint.conversation(agent_id.as_str())?;
        self.connect_url(
            endpoint,
            agent_id,
            url,
            prepared,
            RuntimeWorkloadClass::Ordinary,
        )
        .await
    }

    pub(super) async fn connect_url(
        &self,
        endpoint: &RuntimeEndpoint,
        _agent_id: &AgentSlot,
        mut url: url::Url,
        prepared: Option<crate::agent_sandbox::RuntimeStream>,
        class: RuntimeWorkloadClass,
    ) -> crate::Result<Socket> {
        url.query_pairs_mut().append_pair("outputAck", "1");
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| AdapterError::Rejected("runtime WebSocket URL is invalid".into()))?;
        let (name, value) = self.auth_header(endpoint);
        let mut value: http::HeaderValue = value
            .parse()
            .map_err(|_| AdapterError::Rejected("provider API key is not a valid header".into()))?;
        value.set_sensitive(true);
        request.headers_mut().insert(name, value);
        if let Some(owner) = crate::owner_protocol::current_header()? {
            request.headers_mut().insert(
                crate::owner_protocol::OWNER_HEADER,
                owner
                    .parse()
                    .map_err(|_| AdapterError::Rejected("invalid ownership header".into()))?,
            );
        }
        let config = WebSocketConfig {
            max_message_size: Some(MAX_FRAME_BYTES + 25),
            max_frame_size: Some(MAX_FRAME_BYTES + 25),
            ..WebSocketConfig::default()
        };
        if endpoint.native_binding().is_some() {
            let connected = async {
                let stream = match prepared {
                    Some(stream) => stream,
                    None => {
                        self.native_stream(
                            endpoint,
                            if class == RuntimeWorkloadClass::Continuity {
                                RuntimeStreamKind::ContinuityConversation
                            } else {
                                RuntimeStreamKind::Conversation
                            },
                        )
                        .await?
                    }
                };
                let (socket, _) =
                    tokio_tungstenite::client_async_with_config(request, stream, Some(config))
                        .await
                        .map_err(|_| {
                            AdapterError::Unavailable(
                                "runtime WebSocket over Kubernetes failed".into(),
                            )
                        })?;
                Ok(Box::new(socket) as Socket)
            };
            return tokio::time::timeout(CONNECT_DEADLINE, connected)
                .await
                .map_err(|_| {
                    AdapterError::Unavailable("runtime WebSocket connection timed out".into())
                })?;
        }
        let connected = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (stream, _) = tokio::time::timeout(CONNECT_DEADLINE, connected)
            .await
            .map_err(|_| {
                AdapterError::Unavailable("runtime WebSocket connection timed out".into())
            })?
            .map_err(|error| {
                AdapterError::Unavailable(format!("runtime WebSocket connection failed: {error}"))
            })?;
        Ok(Box::new(stream))
    }
}

#[path = "provider_control.rs"]
mod control;
#[path = "provider_ownership_legacy.rs"]
mod legacy_receiver;
#[path = "native_http_pool.rs"]
mod native_pool;
#[path = "provider_open.rs"]
mod open;
#[path = "provider_ownership.rs"]
mod ownership;
pub use legacy_receiver::LegacyReceiverImage;

#[path = "native_status.rs"]
mod native_status;
#[path = "native_transport.rs"]
mod native_transport;

#[path = "provider_transport_impl.rs"]
mod transport_impl;

#[path = "provider_desktop.rs"]
mod desktop;

#[cfg(test)]
#[path = "provider_transport_tests.rs"]
mod tests;

#[path = "provider_discovery.rs"]
mod discovery;

#[path = "provider_history.rs"]
mod history;

#[path = "provider_binding.rs"]
mod binding;

#[path = "provider_capacity_proof.rs"]
mod capacity_proof;
#[path = "provider_stale_turn.rs"]
mod stale_turn;
