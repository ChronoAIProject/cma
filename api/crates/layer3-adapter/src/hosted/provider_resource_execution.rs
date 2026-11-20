use super::*;
use crate::{
    ingress_budget::{IngressBudget, IngressReservation},
    owner_protocol::{self, TransportAuthority},
    resource_execution_protocol::*,
    ResourceConversationTransport, ResourceDispatch, ResourceSendOutcome,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::sync::Semaphore;

const EXCHANGE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilitiesReply {
    protocol: u32,
    receiver: ReceiverKey,
    capabilities: Option<Capabilities>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    protocol: u32,
    targeted_native_stop: bool,
    mediated_effects: bool,
    mixed_principal_control: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Continued {
    protocol: u32,
}

struct ResourceTransport {
    client: ProviderHostedControlTransport,
    endpoint: RuntimeEndpoint,
    authority: Arc<dyn TransportAuthority>,
    receiver: ReceiverKey,
    capabilities: ExecutionCapabilities,
    _ownership: Option<ownership::ReceiverLease>,
}

impl ProviderHostedControlTransport {
    pub(super) async fn bind_resource_execution(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Option<Arc<dyn ResourceConversationTransport>>> {
        let authority = owner_protocol::current_authority().ok_or_else(|| {
            AdapterError::Rejected("resource transport requires explicit manager ownership".into())
        })?;
        if authority.owner().agent_id != agent.as_str() {
            return Err(adapter_error(ProtocolError::AudienceMismatch));
        }
        authority.check().await?;
        let endpoint = self.endpoint(handle).await?;
        let ownership = self.receiver_lease(&endpoint).await?;
        let response: CapabilitiesReply = match resource_exchange(
            self,
            &endpoint,
            &authority,
            "capabilities",
            &serde_json::json!({"protocol": PROTOCOL_VERSION}),
        )
        .await
        {
            Ok(reply) => reply,
            Err(ProtocolError::Unsupported) => return Ok(None),
            Err(error) => return Err(adapter_error(error)),
        };
        version(response.protocol).map_err(adapter_error)?;
        let Some(capabilities) = response.capabilities else {
            return Ok(None);
        };
        if capabilities.protocol != PROTOCOL_VERSION
            || !capabilities.targeted_native_stop
            || !capabilities.mediated_effects
        {
            return Ok(None);
        }
        if response.receiver.kind != ReceiverKind::Runtime {
            return Err(adapter_error(ProtocolError::AudienceMismatch));
        }
        Ok(Some(Arc::new(ResourceTransport {
            client: self.clone(),
            endpoint,
            authority,
            receiver: response.receiver,
            capabilities: ExecutionCapabilities {
                protocol: capabilities.protocol,
                targeted_native_stop: capabilities.targeted_native_stop,
                mediated_effects: capabilities.mediated_effects,
                mixed_principal_control: capabilities.mixed_principal_control,
            },
            _ownership: ownership,
        })))
    }
}

impl ResourceTransport {
    fn audience(&self, execution: &ExecutionKey, current_boot: bool) -> ProtocolResult<()> {
        if execution.agent_id.as_str() != self.authority.owner().agent_id
            || execution.receiver.kind != self.receiver.kind
            || execution.receiver.instance != self.receiver.instance
            || execution.receiver.incarnation != self.receiver.incarnation
            || (current_boot && execution.receiver.boot != self.receiver.boot)
        {
            return Err(ProtocolError::AudienceMismatch);
        }
        Ok(())
    }

    fn barrier_audience(&self, request: &BarrierRequestV1) -> ProtocolResult<()> {
        request.validate()?;
        let MemberKey::Execution { execution } = &request.member else {
            return Err(ProtocolError::AudienceMismatch);
        };
        self.audience(execution, false)
    }

    async fn exchange<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        action: &str,
        request: &T,
    ) -> ProtocolResult<R> {
        resource_exchange(
            &self.client,
            &self.endpoint,
            &self.authority,
            action,
            request,
        )
        .await
    }
}

#[async_trait::async_trait]
impl RuntimeExecutionControl for ResourceTransport {
    fn capabilities(&self) -> Option<ExecutionCapabilities> {
        self.authority.is_current().then_some(self.capabilities)
    }

    async fn challenge(&self, execution: &ExecutionKey) -> ProtocolResult<ReceiverChallengeV1> {
        self.audience(execution, true)?;
        let reply: ReceiverChallengeV1 = self.exchange("challenge", execution).await?;
        reply.validate()?;
        if reply.receiver != self.receiver {
            return Err(ProtocolError::AudienceMismatch);
        }
        Ok(reply)
    }

    async fn continue_execution(&self, lease: &ContinuationV1) -> ProtocolResult<()> {
        self.audience(&lease.execution, true)?;
        version(lease.protocol)?;
        lease.challenge.validate()?;
        if !lease.manager.matches_owner(&self.authority.owner())
            || lease.challenge.receiver != self.receiver
        {
            return Err(ProtocolError::AudienceMismatch);
        }
        let reply: Continued = self.exchange("continue", lease).await?;
        version(reply.protocol)
    }

    async fn install_barrier(
        &self,
        request: &BarrierRequestV1,
    ) -> ProtocolResult<BarrierReceiptV1> {
        self.barrier_audience(request)?;
        let reply: BarrierReceiptV1 = self.exchange("barrier", request).await?;
        reply.validate_for(request)?;
        Ok(reply)
    }

    async fn cancel(&self, request: &BarrierRequestV1) -> ProtocolResult<StopReceiptV1> {
        self.barrier_audience(request)?;
        let reply: StopReceiptV1 = self.exchange("cancel", request).await?;
        reply.validate_for(request)?;
        Ok(reply)
    }

    async fn stop_receipt(
        &self,
        request: &BarrierRequestV1,
    ) -> ProtocolResult<Option<StopReceiptV1>> {
        self.barrier_audience(request)?;
        let reply: Option<StopReceiptV1> = self.exchange("stop", request).await?;
        if let Some(reply) = &reply {
            reply.validate_for(request)?
        }
        Ok(reply)
    }
}

#[async_trait::async_trait]
impl ResourceConversationTransport for ResourceTransport {
    fn receiver(&self) -> Option<&ReceiverKey> {
        Some(&self.receiver)
    }

    async fn send_resource(&self, dispatch: &ResourceDispatch) -> ResourceSendOutcome {
        let ticket = dispatch.ticket();
        let preflight = async {
            self.audience(&ticket.execution, true)?;
            ticket.validate()?;
            if !ticket.manager.matches_owner(&self.authority.owner()) {
                return Err(ProtocolError::AudienceMismatch);
            }
            self.authority
                .check()
                .await
                .map_err(|_| ProtocolError::AudienceMismatch)
        }
        .await;
        if let Err(error) = preflight {
            return ResourceSendOutcome::Refused(adapter_error(error));
        }
        #[derive(Serialize)]
        struct Start<'a> {
            protocol: u32,
            ticket: &'a ExecutionTicketV1,
            input: &'a serde_json::value::RawValue,
        }
        let input = match serde_json::from_slice(dispatch.input().bytes()) {
            Ok(input) => input,
            Err(_) => {
                return ResourceSendOutcome::Refused(adapter_error(ProtocolError::InvalidIdentity))
            }
        };
        let request = Start {
            protocol: PROTOCOL_VERSION,
            ticket,
            input,
        };
        let reply: ProtocolResult<StartObservationV1> = self.exchange("start", &request).await;
        match reply.and_then(|reply| {
            reply.validate_for(ticket)?;
            Ok(reply)
        }) {
            Ok(reply) => ResourceSendOutcome::Observed(Box::new(reply)),
            // No transport error proves the native release did not happen.
            Err(error) => ResourceSendOutcome::Uncertain(adapter_error(error)),
        }
    }
}

async fn resource_exchange<T: Serialize + Sync, R: DeserializeOwned>(
    client: &ProviderHostedControlTransport,
    endpoint: &RuntimeEndpoint,
    authority: &Arc<dyn TransportAuthority>,
    action: &str,
    request: &T,
) -> ProtocolResult<R> {
    static CUSTODY: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let _custody = IngressReservation::try_acquire(
        CUSTODY.get_or_init(|| Arc::new(Semaphore::new(8 * 1024 * 1024))),
        Some(&IngressBudget::process()),
        4 * MAX_MESSAGE_BYTES,
    )
    .ok_or(ProtocolError::Capacity)?;
    let mut encoded = LimitedBody(Vec::new());
    serde_json::to_writer(&mut encoded, request).map_err(|_| ProtocolError::Capacity)?;
    let url = endpoint
        .resource_execution(&authority.owner().agent_id, action)
        .map_err(|_| ProtocolError::InvalidIdentity)?;
    let operation = owner_protocol::scope(authority.clone(), async {
        authority
            .check()
            .await
            .map_err(|_| ProtocolError::AudienceMismatch)?;
        let data = matches!(action, "start" | "challenge");
        let bytes = if endpoint.native_binding().is_some() {
            let reply = client
                .native_http_for(
                    endpoint,
                    http::Method::POST,
                    url,
                    encoded.0,
                    MAX_MESSAGE_BYTES,
                    if data {
                        (
                            native_pool::NativeHttpLane::ResourceData,
                            RuntimeWorkloadClass::Ordinary,
                        )
                    } else {
                        (
                            native_pool::NativeHttpLane::ResourceControl,
                            RuntimeWorkloadClass::Continuity,
                        )
                    },
                )
                .await
                .map_err(|_| ProtocolError::Unknown)?;
            check_status(reply.status.as_u16())?;
            reply.recycle().await.map_err(|_| ProtocolError::Unknown)?
        } else {
            let (name, token) = client.auth_header(endpoint);
            let mut response = client
                .resource_http
                .post(url)
                .header(name, token)
                .header(
                    owner_protocol::OWNER_HEADER,
                    authority
                        .owner()
                        .header()
                        .map_err(|_| ProtocolError::AudienceMismatch)?,
                )
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(encoded.0)
                .send()
                .await
                .map_err(|_| ProtocolError::Unknown)?;
            check_status(response.status().as_u16())?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| ProtocolError::Unknown)? {
                if chunk.len() > MAX_MESSAGE_BYTES.saturating_sub(bytes.len()) {
                    return Err(ProtocolError::Capacity);
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        };
        authority
            .check()
            .await
            .map_err(|_| ProtocolError::Unknown)?;
        decode_message(&bytes)
    });
    tokio::select! { biased;
        _ = authority.revoked() => Err(ProtocolError::Unknown),
        result = tokio::time::timeout(EXCHANGE_DEADLINE, operation) => result.map_err(|_| ProtocolError::Unknown)?,
    }
}

fn check_status(status: u16) -> ProtocolResult<()> {
    match status {
        200 => Ok(()),
        404 | 501 => Err(ProtocolError::Unsupported),
        401 | 403 => Err(ProtocolError::AudienceMismatch),
        400 => Err(ProtocolError::InvalidIdentity),
        413 | 429 => Err(ProtocolError::Capacity),
        _ => Err(ProtocolError::Unknown),
    }
}

fn adapter_error(error: ProtocolError) -> AdapterError {
    if error == ProtocolError::Unsupported {
        AdapterError::Unsupported(error.to_string())
    } else {
        AdapterError::Unavailable(error.to_string())
    }
}

struct LimitedBody(Vec<u8>);
impl std::io::Write for LimitedBody {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_MESSAGE_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("resource request capacity"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "provider_resource_execution_tests.rs"]
mod tests;
