//! HTTP over an authenticated Kubernetes stream; never retry a sent operation.
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;

use super::*;
use crate::agent_sandbox::{RuntimeStream, RuntimeStreamKind};

struct AbortOnDrop(tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
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
    ) -> crate::Result<(http::StatusCode, Vec<u8>)> {
        let stream = self
            .native_stream(endpoint, RuntimeStreamKind::Request)
            .await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| unavailable("runtime HTTP handshake over Kubernetes failed"))?;
        let _driver = AbortOnDrop(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let (name, token) = self.auth_header(endpoint);
        let mut token = http::HeaderValue::from_str(token)
            .map_err(|_| AdapterError::Rejected("runtime token is not a valid header".into()))?;
        token.set_sensitive(true);
        let request = http::Request::builder()
            .method(method)
            .uri(&url[url::Position::BeforePath..])
            .header(
                http::header::HOST,
                &url[url::Position::BeforeHost..url::Position::AfterPort],
            )
            .header(http::header::CONNECTION, "close")
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(name, token)
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| AdapterError::Rejected("runtime HTTP request is invalid".into()))?;
        // One attempt to this fenced Pod. An ambiguous response must never cause
        // an implicit second mutation, or rediscovery/replay on a replacement Pod.
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| unavailable("runtime HTTP exchange over Kubernetes failed"))?;
        let status = response.status();
        if !status.is_success() {
            // A server may echo request credentials in an error body. Only status
            // participates in error classification; never return that body to logs.
            return Ok((status, Vec::new()));
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
        Ok((status, bytes))
    }
}

fn unavailable(message: &str) -> AdapterError {
    AdapterError::Unavailable(message.into())
}

#[cfg(test)]
#[path = "native_transport_tests.rs"]
mod tests;
