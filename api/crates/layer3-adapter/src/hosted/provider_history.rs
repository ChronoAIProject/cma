use super::*;
use futures_util::StreamExt;

const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const HISTORY_TIMEOUT: Duration = Duration::from_secs(120);

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
        let endpoint = self.endpoint(handle).await?;
        let url = endpoint.history(agent.as_str())?;
        if endpoint.native_binding().is_some() {
            let (status, body) = tokio::time::timeout(
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
            .map_err(|_| AdapterError::Unavailable("runtime history request timed out".into()))??;
            return if status.is_success() {
                Ok(body)
            } else {
                Err(AdapterError::Unavailable(format!(
                    "runtime history response failed ({status})"
                )))
            };
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
                AdapterError::Unavailable(format!("runtime history request failed: {error}"))
            })?;
        if !response.status().is_success() {
            return Err(AdapterError::Unavailable(format!(
                "runtime history response failed ({})",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_HISTORY_BYTES as u64)
        {
            return Err(AdapterError::Rejected(
                "runtime history response exceeds its size limit".into(),
            ));
        }
        let mut bytes = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|error| {
                AdapterError::Unavailable(format!("read runtime history response: {error}"))
            })?;
            if chunk.len() > MAX_HISTORY_BYTES.saturating_sub(bytes.len()) {
                return Err(AdapterError::Rejected(
                    "runtime history response exceeds its size limit".into(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
