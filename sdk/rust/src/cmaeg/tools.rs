use super::{Cmaeg, types::*};
use crate::{Result, http::Request};
use reqwest::{Method, StatusCode, header};

impl Cmaeg {
    /// Create bounded delayed-reply authority while the original event is valid.
    pub async fn create_reply_target(&self, event_ref: &str) -> Result<ReplyTargetResponse> {
        let segments = ["v1", "events", event_ref, "reply-target"];
        let mut request = Request::operation(
            Method::POST,
            &segments,
            Some(&serde_json::json!({})),
            StatusCode::OK,
        )?;
        request.root = true;
        self.client.request(request).await
    }
    /// Send once under a durable caller-chosen key. Unknown delivery is never retried automatically.
    pub async fn reply_to_target(
        &self,
        target: &str,
        idempotency_key: &str,
        message: &OutboundMessage,
    ) -> Result<IntentReceipt> {
        let segments = ["v1", "reply-targets", target, "messages"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?
                .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Reply using a channel agent key and an opaque event reference.
    pub async fn reply_to_event(
        &self,
        event_ref: &str,
        message: &OutboundMessage,
    ) -> Result<IntentReceipt> {
        let segments = ["v1", "events", event_ref, "replies"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Request an update; the receipt explicitly reports unsupported transports.
    pub async fn update_message(
        &self,
        message_ref: &str,
        message: &OutboundMessage,
    ) -> Result<IntentReceipt> {
        let segments = ["v1", "messages", message_ref, "update"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?;
        request.root = true;
        self.client.request(request).await
    }
    /// Request typing without assuming the target supports it.
    pub async fn send_typing(&self, event_ref: &str) -> Result<TypingReceipt> {
        let segments = ["v1", "events", event_ref, "typing"];
        let mut request = Request::operation(Method::POST, &segments, None::<&()>, StatusCode::OK)?;
        request.root = true;
        request.headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        self.client.request(request).await
    }
    /// Retrieve full event context from provider persistence with agent-key authorization.
    pub async fn read_event_context(&self, event_ref: &str) -> Result<ContextResponse> {
        let segments = ["v1", "events", event_ref, "context"];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
}
