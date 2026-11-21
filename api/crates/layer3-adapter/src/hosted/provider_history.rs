use super::*;
use futures_util::StreamExt;

const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const HISTORY_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy)]
enum HistoryFailure {
    EndpointUnavailable,
    Unsupported,
    Timeout,
    Capacity,
    TransportUnavailable,
    UpstreamRedirect,
    UpstreamClient,
    UpstreamServer,
    BodyUnavailable,
    TooLarge,
}

fn failed(reason: HistoryFailure, error: AdapterError) -> AdapterError {
    let reason = match &error {
        AdapterError::RuntimeCapacity(_) => HistoryFailure::Capacity,
        AdapterError::Unsupported(_) => HistoryFailure::Unsupported,
        _ => reason,
    };
    tracing::warn!(parent: None, reason = ?reason, "Agent raw history transport failed");
    error
}

fn status_failure(status: http::StatusCode) -> AdapterError {
    let reason = if status.is_redirection() {
        HistoryFailure::UpstreamRedirect
    } else if status.is_client_error() {
        HistoryFailure::UpstreamClient
    } else {
        HistoryFailure::UpstreamServer
    };
    failed(
        reason,
        AdapterError::Unavailable("runtime history response failed".into()),
    )
}

impl ProviderHostedControlTransport {
    pub(super) async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        if agent.as_str().is_empty()
            || matches!(agent.as_str(), "." | "..")
            || agent.as_str().len() > 64
            || !agent
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(AdapterError::Rejected(
                "agent history identity is invalid".into(),
            ));
        }
        let endpoint = self
            .endpoint(handle)
            .await
            .map_err(|error| failed(HistoryFailure::EndpointUnavailable, error))?;
        let url = endpoint
            .history(agent.as_str())
            .map_err(|error| failed(HistoryFailure::Unsupported, error))?;
        if endpoint.native_binding().is_some() {
            let reply = tokio::time::timeout(
                HISTORY_TIMEOUT,
                self.native_http(
                    &endpoint,
                    http::Method::GET,
                    url,
                    Vec::new(),
                    MAX_HISTORY_BYTES,
                ),
            )
            .await
            .map_err(|_| {
                failed(
                    HistoryFailure::Timeout,
                    AdapterError::Unavailable("runtime history request timed out".into()),
                )
            })?
            .map_err(|error| failed(HistoryFailure::TransportUnavailable, error))?;
            if !reply.status.is_success() {
                return Err(status_failure(reply.status));
            }
            return reply
                .recycle()
                .await
                .map_err(|error| failed(HistoryFailure::BodyUnavailable, error));
        }
        let auth = self.auth_header(&endpoint);
        let response = self
            .http
            .get(url)
            .header(auth.0, auth.1)
            .timeout(HISTORY_TIMEOUT)
            .send()
            .await
            .map_err(|error| {
                failed(
                    if error.is_timeout() {
                        HistoryFailure::Timeout
                    } else {
                        HistoryFailure::TransportUnavailable
                    },
                    AdapterError::Unavailable("runtime history request failed".into()),
                )
            })?;
        if !response.status().is_success() {
            return Err(status_failure(response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_HISTORY_BYTES as u64)
        {
            return Err(failed(
                HistoryFailure::TooLarge,
                AdapterError::Rejected("runtime history response exceeds its size limit".into()),
            ));
        }
        let mut bytes = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                failed(
                    if error.is_timeout() {
                        HistoryFailure::Timeout
                    } else {
                        HistoryFailure::BodyUnavailable
                    },
                    AdapterError::Unavailable("read runtime history response failed".into()),
                )
            })?;
            if chunk.len() > MAX_HISTORY_BYTES.saturating_sub(bytes.len()) {
                return Err(failed(
                    HistoryFailure::TooLarge,
                    AdapterError::Rejected(
                        "runtime history response exceeds its size limit".into(),
                    ),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
