//! Conversation resources and the supported streaming Responses chat subset.

use crate::{Client, Error, Result, http::Request, resources::identity, stream};
use reqwest::{Method, StatusCode};
mod events;
mod input;
mod types;
pub use events::*;
pub use input::*;
pub use types::*;

/// Conversation-item pagination using server-issued item identities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemOptions {
    /// Exclusive prior item identity.
    pub after: Option<String>,
    /// Bound from 1 through 100.
    pub limit: Option<u16>,
}

/// Public Responses-compatible conversation and response operations.
pub struct Responses<'a> {
    client: &'a Client,
}

impl Responses<'_> {
    /// Explicitly create a conversation; no automatic retry is attempted.
    pub async fn create_conversation(
        &self,
        body: &CreateConversationRequest,
    ) -> Result<Conversation> {
        self.client
            .operation(Method::POST, &["conversations"], Some(body), StatusCode::OK)
            .await
    }
    /// Read the standard conversation projected from one existing Agent.
    pub async fn read_conversation(&self, id: &str) -> Result<Conversation> {
        let result: Conversation = self.client.get(&["conversations", id], &[]).await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Read durable newest-first conversation items and verify page boundary identities.
    pub async fn list_items(&self, id: &str, options: &ItemOptions) -> Result<ConversationItems> {
        if options
            .limit
            .is_some_and(|limit| !(1..=100).contains(&limit))
            || options.after.as_deref() == Some("")
        {
            return Err(Error::validation(
                "conversation item limit must be 1 to 100 and after must be nonempty",
            ));
        }
        let mut query = Vec::new();
        if let Some(after) = &options.after {
            query.push(("after", after.clone()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        let result: ConversationItems = self
            .client
            .get(&["conversations", id, "items"], &query)
            .await?;
        if result.first_id.as_deref() != result.data.first().map(ConversationItem::id)
            || result.last_id.as_deref() != result.data.last().map(ConversationItem::id)
        {
            return Err(Error::protocol(
                "conversation item boundary identities did not match the page",
            ));
        }
        Ok(result)
    }
    /// Admit one streaming response on an existing Agent; reconnecting cannot resume this stream.
    pub async fn create(&self, body: &CreateResponseRequest) -> Result<ResponseStream> {
        self.submit(body, None).await
    }
    /// Recover an uncertain response using this retained key and identical input, without automatic retry.
    pub async fn create_with_key(
        &self,
        body: &CreateResponseRequest,
        key: &str,
    ) -> Result<ResponseStream> {
        self.submit(body, Some(key)).await
    }
    async fn submit(
        &self,
        body: &CreateResponseRequest,
        key: Option<&str>,
    ) -> Result<ResponseStream> {
        if body.conversation.trim().is_empty()
            || body.input.is_empty()
            || !body.stream
            || body.store == Some(false)
            || body.metadata.len() > 16
            || body
                .metadata
                .iter()
                .any(|(key, value)| key.is_empty() || key.len() > 64 || value.len() > 512)
        {
            return Err(Error::validation(
                "Responses requires an existing conversation, one user message, streaming, and durable storage",
            ));
        }
        let request = Request::operation(Method::POST, &["responses"], Some(body), StatusCode::OK)?
            .with_command_key(key)?;
        Ok(ResponseStream {
            inner: stream::open(&self.client.inner, request).await?,
            sequence: 0,
            ended: false,
        })
    }
    /// Reconstruct one response from durable Agent conversation history.
    pub async fn read(&self, id: &str) -> Result<ResponseObject> {
        let result: ResponseObject = self.client.get(&["responses", id], &[]).await?;
        identity(&result.id, id)?;
        Ok(result)
    }
    /// Receive the named unsupported-delete refusal; this API cannot delete durable turns.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.client
            .refusal(Request::operation(
                Method::DELETE,
                &["responses", id],
                None::<&()>,
                StatusCode::OK,
            )?)
            .await
    }
    /// Receive the named unsupported-cancel refusal; use Agent interruption where supported.
    pub async fn cancel(&self, id: &str) -> Result<()> {
        self.client
            .refusal(Request::operation(
                Method::POST,
                &["responses", id, "cancel"],
                None::<&()>,
                StatusCode::OK,
            )?)
            .await
    }
}

impl Client {
    /// Access the Responses-compatible chat subset and durable conversation resources.
    pub fn responses(&self) -> Responses<'_> {
        Responses { client: self }
    }
}
