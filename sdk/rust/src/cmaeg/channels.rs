use super::{Cmaeg, types::*};
use crate::{Result, http::Request};
use reqwest::{Method, StatusCode};

impl Cmaeg {
    /// List owner-visible channels with an opaque continuation cursor.
    pub async fn list_channels(&self, after: Option<&str>) -> Result<ChannelPage> {
        let query: Vec<_> = after
            .map(|value| ("after", value.to_owned()))
            .into_iter()
            .collect();
        let mut request = Request::get(&["v1", "channels"]);
        request.root = true;
        request.query = &query;
        self.client.request(request).await
    }

    /// Create a channel; reserve `record_id` for recovery across replicas/restarts.
    pub async fn create_channel(
        &self,
        body: &ChannelCreateRequest,
        idempotency_key: Option<&str>,
    ) -> Result<ChannelResponse> {
        let mut request = Request::operation(
            Method::POST,
            &["v1", "channels"],
            Some(body),
            StatusCode::CREATED,
        )?
        .with_command_key(idempotency_key)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Read the owner's channel without parsing or replacing its opaque identifier.
    pub async fn get_channel(&self, id: &str) -> Result<ChannelResponse> {
        let segments = ["v1", "channels", id];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
    /// Recover the exact channel after an uncertain reserved-record creation.
    pub async fn recover_channel(&self, record_id: &str) -> Result<ChannelResponse> {
        let segments = ["v1", "channel-records", record_id];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
    /// Replace the complete policy with an optimistic expected record version.
    pub async fn update_channel(
        &self,
        id: &str,
        body: &ChannelUpdateRequest,
    ) -> Result<ChannelResponse> {
        let segments = ["v1", "channels", id];
        let mut request = Request::operation(Method::PUT, &segments, Some(body), StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Delete the expected record version. An uncertain result is not retried.
    pub async fn delete_channel(&self, id: &str, expected_version: u64) -> Result<()> {
        let segments = ["v1", "channels", id];
        let query = [("expected_version", expected_version.to_string())];
        let mut request = Request::operation(
            Method::DELETE,
            &segments,
            None::<&()>,
            StatusCode::NO_CONTENT,
        )?;
        request.root = true;
        request.query = &query;
        self.client.empty(request).await
    }
    /// Run the gateway's synthetic provider test, without sending a channel message.
    /// Defaults to 125 seconds for startup and completion; an explicit builder
    /// request timeout takes precedence.
    pub async fn test_channel(&self, id: &str) -> Result<TestChannelResponse> {
        let segments = ["v1", "channels", id, "test"];
        let mut request = Request::operation(Method::POST, &segments, None::<&()>, StatusCode::OK)?;
        request.root = true;
        request.timeout = Some(self.test_timeout);
        self.client.request(request).await
    }
    /// Re-encrypt the record. Requires a dedicated CMA service credential.
    pub async fn rekey_channel(&self, id: &str) -> Result<RekeyResponse> {
        let segments = ["v1", "channels", id, "rekey"];
        let mut request = Request::operation(Method::POST, &segments, None::<&()>, StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
}
