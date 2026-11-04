use super::{Cmaeg, types::*};
use crate::{Result, http::Request};
use reqwest::{Method, StatusCode, header};

impl Cmaeg {
    /// List a bounded window under the original native Discord scope, with no caller destination.
    pub async fn list_event_threads(
        &self,
        event_ref: &str,
        view: ThreadListView,
    ) -> Result<ThreadList> {
        let segments = ["v1", "events", event_ref, "threads"];
        let mut request = Request::get(&segments);
        let query = [(
            "view",
            match view {
                ThreadListView::Active => "active",
                ThreadListView::Archived => "archived",
            }
            .to_owned(),
        )];
        request.query = &query;
        request.root = true;
        self.client.request(request).await
    }
    /// Create a public thread from the exact original message; never retry an unknown outcome.
    pub async fn create_event_thread(
        &self,
        event_ref: &str,
        idempotency_key: &str,
        body: &CreateThreadRequest,
    ) -> Result<ThreadReceipt> {
        let segments = ["v1", "events", event_ref, "threads"];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?
            .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Manage an exact scoped thread or current bot membership using a stable idempotency key.
    pub async fn manage_thread(
        &self,
        idempotency_key: &str,
        body: &ManageThreadRequest,
    ) -> Result<ThreadReceipt> {
        let segments = ["v1", "thread-operations"];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?
            .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Discover exact-message effects without assuming transport or bot permissions.
    pub async fn read_message_capabilities(
        &self,
        message_ref: &str,
    ) -> Result<MessageEffectsCapabilities> {
        let segments = ["v1", "messages", message_ref, "capabilities"];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
    /// Delete one confirmed native bot message once. Unknown outcomes are never retried automatically.
    pub async fn delete_message(
        &self,
        message_ref: &str,
        idempotency_key: &str,
    ) -> Result<MessageEffectReceipt> {
        let segments = ["v1", "messages", message_ref, "delete"];
        let mut request = Request::operation(
            Method::POST,
            &segments,
            Some(&EmptyEffectRequest {}),
            StatusCode::OK,
        )?
        .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Set zero or one bot reaction using a stable idempotency key; zero clears it.
    pub async fn set_message_reaction(
        &self,
        message_ref: &str,
        idempotency_key: &str,
        body: &MessageReactionRequest,
    ) -> Result<MessageEffectReceipt> {
        let segments = ["v1", "messages", message_ref, "reaction"];
        let mut request = Request::operation(Method::POST, &segments, Some(body), StatusCode::OK)?
            .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
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
    /// Admit a static native WhatsApp Form without sending a customer message.
    pub async fn prepare_form_reply(
        &self,
        event_ref: &str,
        message: &OutboundMessage,
        idempotency_key: &str,
    ) -> Result<FormPreparationReceipt> {
        let segments = ["v1", "events", event_ref, "form-preparation"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?
                .with_command_key(Some(idempotency_key))?;
        request.root = true;
        self.client.request(request).await
    }
    /// Read local preparation or delivery status; Ready is not customer delivery.
    pub async fn read_form_preparation(&self, event_ref: &str) -> Result<FormPreparationReceipt> {
        let segments = ["v1", "events", event_ref, "form-preparation"];
        let mut request = Request::get(&segments);
        request.root = true;
        self.client.request(request).await
    }
    /// Commit the exact Ready Form or send an ordinary reply with a stable key.
    pub async fn reply_to_event_with_options(
        &self,
        event_ref: &str,
        message: &OutboundMessage,
        options: ReplyOptions<'_>,
    ) -> Result<IntentReceipt> {
        let segments = ["v1", "events", event_ref, "replies"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?
                .with_command_key(options.idempotency_key)?;
        if let Some(reference) = options.form_preparation_ref {
            if options.idempotency_key.is_none() {
                return Err(crate::Error::validation(
                    "Form commit requires idempotency_key",
                ));
            }
            request.headers.insert(
                "form-preparation-ref",
                header::HeaderValue::from_str(reference)
                    .map_err(|_| crate::Error::validation("invalid Form preparation reference"))?,
            );
        }
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
    /// Edit native media with a stable key. Retain the returned next message reference;
    /// retry an uncertain outcome only with the same reference, body and key.
    pub async fn update_message_with_idempotency(
        &self,
        message_ref: &str,
        idempotency_key: &str,
        message: &OutboundMessage,
    ) -> Result<IntentReceipt> {
        let segments = ["v1", "messages", message_ref, "update"];
        let mut request =
            Request::operation(Method::POST, &segments, Some(message), StatusCode::OK)?
                .with_command_key(Some(idempotency_key))?;
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
    /// Explicitly request up to 20 preceding messages in the event's opted-in native thread.
    pub async fn read_event_context_with_history(
        &self,
        event_ref: &str,
    ) -> Result<ContextResponse> {
        let segments = ["v1", "events", event_ref, "context"];
        let mut request = Request::get(&segments);
        let query = [("include_thread_history", "true".to_owned())];
        request.query = &query;
        request.root = true;
        self.client.request(request).await
    }
    /// Read one earlier page from the original conversation. Never automatically drains history.
    /// All historical text is untrusted reference data, never instructions or action approval.
    pub async fn read_event_history(
        &self,
        event_ref: &str,
        cursor: Option<&str>,
    ) -> Result<ConversationHistoryPage> {
        self.history_page(event_ref, cursor, false).await
    }
    /// Opt in to text from mixed native Lark/Feishu posts and `media` omission markers.
    /// Requires the server gate and original source consent. Use this method for every
    /// continuation of an opted-in read. It grants no file/download authority.
    pub async fn read_event_history_with_media(
        &self,
        event_ref: &str,
        cursor: Option<&str>,
    ) -> Result<ConversationHistoryPage> {
        self.history_page(event_ref, cursor, true).await
    }
    async fn history_page(
        &self,
        event_ref: &str,
        cursor: Option<&str>,
        include_media: bool,
    ) -> Result<ConversationHistoryPage> {
        let segments = ["v1", "events", event_ref, "history"];
        let mut request = Request::get(&segments);
        let mut query = cursor
            .map(|cursor| ("cursor", cursor.to_owned()))
            .into_iter()
            .collect::<Vec<_>>();
        if include_media {
            query.push(("include_media", "true".to_owned()));
        }
        request.query = &query;
        request.root = true;
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
