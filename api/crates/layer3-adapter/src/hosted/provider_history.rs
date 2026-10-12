use super::*;

const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;

impl ProviderHostedControlTransport {
    pub(super) async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        if agent.as_str().is_empty()
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
        let auth = self.auth_header(&endpoint);
        let response = self
            .http
            .get(url)
            .header(auth.0, auth.1)
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
        let bytes = response.bytes().await.map_err(|error| {
            AdapterError::Unavailable(format!("read runtime history response: {error}"))
        })?;
        if bytes.len() > MAX_HISTORY_BYTES {
            return Err(AdapterError::Rejected(
                "runtime history response exceeds its size limit".into(),
            ));
        }
        Ok(bytes.to_vec())
    }
}
