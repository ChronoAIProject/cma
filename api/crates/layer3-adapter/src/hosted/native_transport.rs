//! HTTP over an authenticated Kubernetes stream; never retry a sent operation.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;

use super::*;
use crate::agent_sandbox::{RuntimeStream, RuntimeStreamKind};
use crate::owner_protocol::ReceiverSettlementFailure as SettlementFailure;

use super::native_pool::{AbortOnDrop, NativeConnection, NativeLease};

// Observations stay with the waiting caller even when a body read is cancelled.
// Other native HTTP users retain the same request and error contract.
#[derive(Default)]
pub(super) struct NativeHttpObservation {
    status: std::sync::atomic::AtomicU16,
    oversized: std::sync::atomic::AtomicBool,
    settlement_origin: std::sync::Mutex<Option<SettlementFailure>>,
}

tokio::task_local! {
    static HTTP_OBSERVATION: Arc<NativeHttpObservation>;
}

impl NativeHttpObservation {
    pub(super) fn status(&self) -> Option<u16> {
        let status = self.status.load(std::sync::atomic::Ordering::Acquire);
        (status != 0).then_some(status)
    }

    pub(super) fn record_status(&self, status: u16) {
        self.status
            .store(status, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn clear(&self) {
        self.status.store(0, std::sync::atomic::Ordering::Release);
        *self.settlement_origin.lock().unwrap() = None;
        self.oversized
            .store(false, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn oversized(&self) -> bool {
        self.oversized.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) async fn observe<F: std::future::Future>(
        self: &Arc<Self>,
        operation: F,
    ) -> F::Output {
        HTTP_OBSERVATION.scope(self.clone(), operation).await
    }

    pub(super) fn settlement_origin(&self) -> Option<SettlementFailure> {
        *self.settlement_origin.lock().unwrap()
    }

    fn record_origin(origin: SettlementFailure) {
        let _ = HTTP_OBSERVATION.try_with(|observation| {
            *observation.settlement_origin.lock().unwrap() = Some(origin);
        });
    }

    fn record_http_failure(error: &hyper::Error) {
        Self::record_origin(if error.is_parse() || error.is_user() {
            SettlementFailure::Invalid
        } else if error.is_closed() || error.is_incomplete_message() || error.is_timeout() {
            SettlementFailure::Unavailable
        } else {
            SettlementFailure::Unclassified
        });
    }

    fn record_oversized() {
        let _ = HTTP_OBSERVATION.try_with(|observation| {
            observation
                .oversized
                .store(true, std::sync::atomic::Ordering::Release);
        });
    }
}

pub(super) struct NativeHttpResponse {
    pub status: http::StatusCode,
    pub bytes: Vec<u8>,
    pub content_type: Option<http::HeaderValue>,
    lease: NativeLease,
}

impl NativeHttpResponse {
    pub async fn recycle(self) -> crate::Result<Vec<u8>> {
        self.lease.recycle().await?;
        Ok(self.bytes)
    }
}

impl ProviderHostedControlTransport {
    pub(super) async fn native_stream(
        &self,
        endpoint: &RuntimeEndpoint,
        kind: RuntimeStreamKind,
    ) -> crate::Result<RuntimeStream> {
        let (RuntimeDiscovery::AgentSandbox(client), Some((handle, binding))) =
            (&self.client, endpoint.native_binding())
        else {
            return Err(AdapterError::Rejected(
                "runtime stream has no native authority".into(),
            ));
        };
        client
            .runtime_stream(handle, binding, kind, self.native_transport)
            .await
    }

    async fn native_request_stream(
        &self,
        endpoint: &RuntimeEndpoint,
        permit: &crate::agent_sandbox::RuntimePermit,
    ) -> crate::Result<RuntimeStream> {
        let (RuntimeDiscovery::AgentSandbox(client), Some((handle, binding))) =
            (&self.client, endpoint.native_binding())
        else {
            return Err(AdapterError::Rejected(
                "runtime request has no native authority".into(),
            ));
        };
        client
            .runtime_request_stream(handle, binding, self.native_transport, permit)
            .await
    }

    pub(super) async fn native_http(
        &self,
        endpoint: &RuntimeEndpoint,
        method: http::Method,
        url: url::Url,
        body: Vec<u8>,
        limit: usize,
    ) -> crate::Result<NativeHttpResponse> {
        self.native_http_for(
            endpoint,
            method,
            url,
            body,
            limit,
            (
                native_pool::NativeHttpLane::Control,
                RuntimeWorkloadClass::Ordinary,
            ),
        )
        .await
    }

    pub(super) async fn native_http_for(
        &self,
        endpoint: &RuntimeEndpoint,
        method: http::Method,
        url: url::Url,
        body: Vec<u8>,
        limit: usize,
        policy: (native_pool::NativeHttpLane, RuntimeWorkloadClass),
    ) -> crate::Result<NativeHttpResponse> {
        let Some((runtime, pod)) = endpoint.native_binding() else {
            return Err(native_pool::retired());
        };
        let digest = endpoint
            .native_identity()
            .and_then(|identity| identity.request_sha256.as_deref());
        let (lease, _request) =
            crate::agent_sandbox::relay_readiness_retry::prepare(runtime, pod, digest, |fresh| {
                Box::pin(self.prepare_native_http(endpoint, policy, fresh))
            })
            .await?;
        let mut cancelled = lease.slot.cancellation();
        let cancellation = async {
            // Release the watch read guard before select drops the lease.
            // Dropping the last slot owner writes to this same watch channel.
            let _ = cancelled.wait_for(|cancelled| *cancelled).await;
        };
        tokio::select! { biased;
            _ = cancellation => Err(cancelled_exchange()),
            result = self.native_http_once(endpoint, method, url, body, limit, lease) => result,
        }
    }

    async fn prepare_native_http(
        &self,
        endpoint: &RuntimeEndpoint,
        policy: (native_pool::NativeHttpLane, RuntimeWorkloadClass),
        fresh: bool,
    ) -> crate::Result<(NativeLease, crate::agent_sandbox::RuntimePermit)> {
        let (lane, class) = policy;
        let class = class.effective();
        let request = self
            .runtime_capacity
            .acquire_request(class)
            .await
            .inspect_err(|_| {
                NativeHttpObservation::record_origin(SettlementFailure::Unavailable);
            })?;
        let slot = self
            .native_slot_for_class(endpoint, lane, class)
            .await
            .inspect_err(|error| {
                if matches!(error, AdapterError::RuntimeCapacity(_)) {
                    NativeHttpObservation::record_origin(SettlementFailure::Unavailable);
                }
            })?;
        let mut cancelled = slot.cancellation();
        let exchange = async {
            let mut lease = self.native_checkout(endpoint, slot.clone()).await?;
            if fresh
                || lease
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.sender.is_closed())
            {
                lease.connection.take();
            }
            if lease.connection.is_some() {
                let (RuntimeDiscovery::AgentSandbox(client), Some((handle, binding))) =
                    (&self.client, endpoint.native_binding())
                else {
                    return Err(native_pool::retired());
                };
                // A queued request must obtain its fresh proof after checkout.
                let current = match client.pod_binding(handle).await {
                    Ok(current) => current,
                    Err(error) => {
                        if matches!(&error, AdapterError::Rejected(_)) {
                            self.discard_native_incarnation(endpoint, &slot).await;
                        } else {
                            self.discard_native_slot(endpoint, &slot).await;
                        }
                        return Err(error);
                    }
                };
                if current != *binding {
                    NativeHttpObservation::record_origin(SettlementFailure::AuthorityLost);
                    self.discard_native_incarnation(endpoint, &slot).await;
                    return Err(AdapterError::Rejected(
                        "Sandbox Pod changed before HTTP reuse".into(),
                    ));
                }
            } else {
                // Opening a new stream retains the original pre/post-open fences.
                if fresh {
                    let RuntimeDiscovery::AgentSandbox(client) = &self.client else {
                        return Err(native_pool::retired());
                    };
                    crate::agent_sandbox::relay_readiness_retry::verify_reconnect(client).await?;
                    lease.current().await?;
                }
                let stream = match self.native_request_stream(endpoint, &request).await {
                    Ok(stream) => stream,
                    Err(error) => {
                        // New-stream Pod admission can reject the captured
                        // incarnation before this lane ever has a connection.
                        // An authoritative rejection fences its other lanes too;
                        // capacity/transport failures stay local to this lane.
                        if matches!(&error, AdapterError::Rejected(_)) {
                            self.discard_native_incarnation(endpoint, &slot).await;
                        } else {
                            self.discard_native_slot(endpoint, &slot).await;
                        }
                        return Err(error);
                    }
                };
                let (sender, connection) =
                    hyper::client::conn::http1::handshake(TokioIo::new(stream))
                        .await
                        .map_err(|error| {
                            NativeHttpObservation::record_http_failure(&error);
                            crate::workspace_prepare_observation::native_transport();
                            unavailable("runtime HTTP handshake over Kubernetes failed")
                        })?;
                let mut retired = slot.cancellation();
                let driver = tokio::spawn(async move {
                    tokio::select! { biased;
                        _ = retired.wait_for(|cancelled| *cancelled) => {},
                        _ = connection => {},
                    }
                });
                lease.connection = Some(NativeConnection {
                    sender,
                    _driver: AbortOnDrop(driver),
                });
            }
            lease.current().await?;
            if lane == native_pool::NativeHttpLane::CloseAgent {
                // A close can wait for an earlier close on this connection. Its
                // negotiated owner must still be current at the send boundary.
                crate::owner_protocol::current_authority()
                    .ok_or_else(native_pool::retired)?
                    .check()
                    .await?;
                lease.current().await?;
            }
            Ok(lease)
        };
        let lease = tokio::select! { biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => Err(cancelled_exchange()),
            result = exchange => result,
        }?;
        Ok((lease, request))
    }

    pub(super) async fn native_http_once(
        &self,
        endpoint: &RuntimeEndpoint,
        method: http::Method,
        url: url::Url,
        body: Vec<u8>,
        limit: usize,
        mut lease: NativeLease,
    ) -> crate::Result<NativeHttpResponse> {
        let (name, token) = self.auth_header(endpoint);
        let mut token = http::HeaderValue::from_str(token).map_err(|_| {
            NativeHttpObservation::record_origin(SettlementFailure::Invalid);
            AdapterError::Rejected("runtime token is not a valid header".into())
        })?;
        token.set_sensitive(true);
        let mut request = http::Request::builder()
            .method(method)
            .uri(&url[url::Position::BeforePath..])
            .header(
                http::header::HOST,
                &url[url::Position::BeforeHost..url::Position::AfterPort],
            )
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(
                http::header::ACCEPT,
                endpoint.accept().map_or_else(
                    || {
                        format!(
                            "{}, {}, {}, {}, {}, {}",
                            crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
                            crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
                            crate::wire::AGENT_PROCESS_RESTART_STATUS_MEDIA_TYPE,
                            crate::wire::AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
                            crate::wire::GITHUB_CREDENTIAL_ROUTE_STATUS_MEDIA_TYPE,
                            crate::wire::GITHUB_REPOSITORY_SCOPE_STATUS_MEDIA_TYPE,
                        )
                    },
                    str::to_owned,
                ),
            )
            .header(name, token);
        if let Some(owner) = crate::owner_protocol::current_header()? {
            request = request.header(crate::owner_protocol::OWNER_HEADER, owner);
        }
        let request = request.body(Full::new(Bytes::from(body))).map_err(|_| {
            NativeHttpObservation::record_origin(SettlementFailure::Invalid);
            AdapterError::Rejected("runtime HTTP request is invalid".into())
        })?;
        // One attempt to this fenced Pod. An ambiguous response must never cause
        // an implicit second mutation, or rediscovery/replay on a replacement Pod.
        let response = lease
            .connection
            .as_mut()
            .expect("checked-out connection")
            .sender
            .send_request(request)
            .await
            .map_err(|error| {
                NativeHttpObservation::record_http_failure(&error);
                crate::workspace_prepare_observation::native_transport();
                unavailable("runtime HTTP exchange over Kubernetes failed")
            })?;
        let status = response.status();
        let _ = HTTP_OBSERVATION.try_with(|observation| observation.record_status(status.as_u16()));
        if !status.is_success() {
            // A server may echo request credentials in an error body. Only status
            // participates in error classification; never return that body to logs.
            return Ok(NativeHttpResponse {
                status,
                bytes: Vec::new(),
                content_type: None,
                lease,
            });
        }
        if response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > limit as u64)
        {
            NativeHttpObservation::record_oversized();
            return Err(AdapterError::Rejected(
                "runtime HTTP response exceeds the size limit".into(),
            ));
        }
        let content_type = endpoint
            .accept()
            .and_then(|_| response.headers().get(http::header::CONTENT_TYPE).cloned());
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| {
                NativeHttpObservation::record_http_failure(&error);
                crate::workspace_prepare_observation::native_transport();
                unavailable("runtime HTTP body over Kubernetes failed")
            })?;
            if let Some(data) = frame.data_ref() {
                if data.len() > limit.saturating_sub(bytes.len()) {
                    NativeHttpObservation::record_oversized();
                    return Err(AdapterError::Rejected(
                        "runtime HTTP response exceeds the size limit".into(),
                    ));
                }
                bytes.extend_from_slice(data);
            }
        }
        Ok(NativeHttpResponse {
            status,
            bytes,
            content_type,
            lease,
        })
    }
}

fn unavailable(message: &str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

#[cfg(test)]
#[path = "native_transport_tests.rs"]
mod tests;

fn cancelled_exchange() -> AdapterError {
    crate::agent_sandbox::relay_readiness_retry::clear_marker();
    NativeHttpObservation::record_origin(SettlementFailure::Unclassified);
    native_pool::retired()
}
