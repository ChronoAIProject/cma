//! Versioned Responses subset with durable admission receipts and explicit replay.
use crate::{
    Client, Error, Result,
    http::{self, Request},
    resources::identity,
};
use reqwest::{Method, StatusCode, header::HeaderValue};
use serde::de::DeserializeOwned;
mod activity_state;
mod controls;
mod controls_types;
mod event_context;
mod events;
pub use event_context::EventContext;
mod input;
mod interruption_types;
mod queued_cancellation_types;
mod queued_input_types;
mod resources;
mod state;
mod stream;
mod types;
pub use controls_types::*;
pub use events::*;
pub use input::*;
pub use interruption_types::*;
pub use queued_cancellation_types::*;
pub use queued_input_types::*;
pub use resources::*;
pub use stream::*;
pub use types::*;

/// Responses v2 operations; v1 remains available through `Client::responses()`.
pub struct ResponsesV2<'a> {
    client: &'a Client,
}
impl ResponsesV2<'_> {
    /// Discover the supported subset and CMA replay/AG-UI companion paths.
    pub async fn read_capabilities(&self) -> Result<Capabilities> {
        self.get(&["api", "v2"], &[]).await
    }
    /// Read the current durable response under fresh history authorization.
    pub async fn read(&self, id: &str) -> Result<ResponseObject> {
        let response: ResponseObject = self.get(&["api", "v2", "responses", id], &[]).await?;
        identity(&response.id, id)?;
        Ok(response)
    }
    /// Read an existing Agent conversation.
    pub async fn read_conversation(&self, id: &str) -> Result<Conversation> {
        let conversation: Conversation = self.get(&["api", "v2", "conversations", id], &[]).await?;
        if id.starts_with("agt_") {
            identity(&conversation.id, id)?;
        }
        Ok(conversation)
    }
    /// Read committed standard messages with exclusive cursor pagination.
    pub async fn list_items(&self, id: &str, options: &ItemOptions) -> Result<ConversationItems> {
        if options
            .limit
            .is_some_and(|limit| !(1..=100).contains(&limit))
        {
            return Err(Error::validation("limit must be 1 through 100"));
        }
        let mut query = Vec::new();
        if let Some(after) = &options.after {
            query.push(("after", after.clone()));
        }
        if let Some(before) = &options.before {
            query.push(("before", before.clone()));
        }
        if let Some(order) = &options.order {
            query.push(("order", order.as_str().into()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        let page: ConversationItems = self
            .get(&["api", "v2", "conversations", id, "items"], &query)
            .await?;
        if page.first_id.as_deref() != page.data.first().map(ConversationItem::id)
            || page.last_id.as_deref() != page.data.last().map(ConversationItem::id)
            || page
                .data
                .iter()
                .map(ConversationItem::id)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != page.data.len()
        {
            return Err(Error::protocol(
                "conversation page boundaries or identities are inconsistent",
            ));
        }
        Ok(page)
    }
    /// Submit once. Persist a stable key before sending to recover a lost first reply.
    pub async fn create(
        &self,
        body: &CreateResponseRequest,
        idempotency_key: Option<&str>,
    ) -> Result<ResponseStream> {
        self.submit(&["api", "v2", "responses"], body, idempotency_key, None)
            .await
    }
    /// Equivalent per-Agent alias; any supplied conversation must match.
    pub async fn create_for_agent(
        &self,
        id: &str,
        body: &CreateResponseRequest,
        idempotency_key: Option<&str>,
    ) -> Result<ResponseStream> {
        self.submit(
            &["api", "v2", "agents", id, "responses"],
            body,
            idempotency_key,
            Some(id),
        )
        .await
    }
    /// Explicit replay after an acknowledged durable event; never resubmits input.
    pub async fn replay(&self, id: &str, starting_after: Option<u64>) -> Result<ResponseStream> {
        if starting_after == Some(u64::MAX) {
            return Err(Error::validation("replay cursor overflow"));
        }
        let segments = ["api", "v2", "responses", id, "events"];
        let query: Vec<_> = starting_after
            .map(|n| ("starting_after", n.to_string()))
            .into_iter()
            .collect();
        let mut request = Request::get(&segments);
        request.root = true;
        request.query = &query;
        ResponseStream::new(
            crate::stream::open(&self.client.inner, request).await?,
            None,
            Some(id),
            starting_after,
        )
    }
    async fn submit(
        &self,
        segments: &[&str],
        body: &CreateResponseRequest,
        key: Option<&str>,
        alias: Option<&str>,
    ) -> Result<ResponseStream> {
        let conversation = body.validate(alias)?;
        let mut request = Request::operation(Method::POST, segments, Some(body), StatusCode::OK)?;
        request.root = true;
        if let Some(key) = key {
            if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
                return Err(Error::validation(
                    "idempotency key must contain 1 through 256 bytes without control characters",
                ));
            }
            request.headers.insert(
                "idempotency-key",
                HeaderValue::from_str(key)
                    .map_err(|_| Error::validation("idempotency key is not a valid HTTP header"))?,
            );
        }
        ResponseStream::new(
            crate::stream::open(&self.client.inner, request).await?,
            alias
                .filter(|id| id.starts_with("agt_"))
                .map(str::to_owned)
                .or_else(|| conversation.starts_with("agt_").then_some(conversation)),
            None,
            None,
        )
    }
    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T> {
        let mut request = Request::get(segments);
        request.root = true;
        request.query = query;
        let body = http::send(&self.client.inner, request).await?;
        serde_json::from_slice(&body)
            .map_err(|_| Error::protocol("invalid Responses v2 resource document"))
    }
}
impl Client {
    /// Access versioned durable Responses operations without changing the v1 adapter.
    pub fn responses_v2(&self) -> ResponsesV2<'_> {
        ResponsesV2 { client: self }
    }
}
