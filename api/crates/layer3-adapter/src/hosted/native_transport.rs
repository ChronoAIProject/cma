//! HTTP over an authenticated Kubernetes stream; never retry a sent operation.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;

use super::*;
use crate::agent_sandbox::{RuntimeStream, RuntimeStreamKind};

use super::native_pool::{AbortOnDrop, NativeConnection, NativeLease};

pub(super) struct NativeHttpResponse {
    pub status: http::StatusCode,
    pub bytes: Vec<u8>,
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

    pub(super) async fn native_http(
        &self,
        endpoint: &RuntimeEndpoint,
        method: http::Method,
        url: url::Url,
        body: Vec<u8>,
        limit: usize,
    ) -> crate::Result<NativeHttpResponse> {
        let _request = self
            .native_http_budget
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                AdapterError::AtCapacity("native HTTP request capacity is exhausted".into())
            })?;
        let slot = self.native_slot(endpoint).await?;
        let mut cancelled = slot.cancellation();
        let exchange = async {
            let mut lease = self.native_checkout(endpoint, slot.clone()).await?;
            if lease
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
                        self.discard_native_slot(endpoint, &slot).await;
                        return Err(error);
                    }
                };
                if current != *binding {
                    self.discard_native_slot(endpoint, &slot).await;
                    return Err(AdapterError::Rejected(
                        "Sandbox Pod changed before HTTP reuse".into(),
                    ));
                }
            } else {
                // Opening a new stream retains the original pre/post-open fences.
                let stream = match self
                    .native_stream(endpoint, RuntimeStreamKind::Request)
                    .await
                {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.discard_native_slot(endpoint, &slot).await;
                        return Err(error);
                    }
                };
                let (sender, connection) =
                    hyper::client::conn::http1::handshake(TokioIo::new(stream))
                        .await
                        .map_err(|_| {
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
            self.native_http_once(endpoint, method, url, body, limit, lease)
                .await
        };
        tokio::select! { biased;
            _ = cancelled.wait_for(|cancelled| *cancelled) => Err(native_pool::retired()),
            result = exchange => result,
        }
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
        let mut token = http::HeaderValue::from_str(token)
            .map_err(|_| AdapterError::Rejected("runtime token is not a valid header".into()))?;
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
                format!(
                    "{}, {}",
                    crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
                    crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
                ),
            )
            .header(name, token);
        if let Some(owner) = crate::owner_protocol::current_header()? {
            request = request.header(crate::owner_protocol::OWNER_HEADER, owner);
        }
        let request = request
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| AdapterError::Rejected("runtime HTTP request is invalid".into()))?;
        // One attempt to this fenced Pod. An ambiguous response must never cause
        // an implicit second mutation, or rediscovery/replay on a replacement Pod.
        let response = lease
            .connection
            .as_mut()
            .expect("checked-out connection")
            .sender
            .send_request(request)
            .await
            .map_err(|_| unavailable("runtime HTTP exchange over Kubernetes failed"))?;
        let status = response.status();
        if !status.is_success() {
            // A server may echo request credentials in an error body. Only status
            // participates in error classification; never return that body to logs.
            return Ok(NativeHttpResponse {
                status,
                bytes: Vec::new(),
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
            return Err(AdapterError::Rejected(
                "runtime HTTP response exceeds the size limit".into(),
            ));
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame =
                frame.map_err(|_| unavailable("runtime HTTP body over Kubernetes failed"))?;
            if let Some(data) = frame.data_ref() {
                if data.len() > limit.saturating_sub(bytes.len()) {
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
