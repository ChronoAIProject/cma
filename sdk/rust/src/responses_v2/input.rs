//! Closed single-message input forms for Responses v2.
use crate::{
    Error, Result,
    responses::{InputMessageKind, InputRole, InputText},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Existing Agent or binding conversation selected by string or reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConversationReference {
    /// Agent ID or external binding conversation ID.
    Id(String),
    /// Standard reference object.
    Object(super::ResponseConversation),
}
impl ConversationReference {
    /// Selected Agent or external conversation ID.
    pub fn id(&self) -> &str {
        match self {
            Self::Id(id) => id,
            Self::Object(value) => &value.id,
        }
    }
}
/// Adjacent text parts are concatenated by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputContent {
    /// Plain text.
    Text(String),
    /// Adjacent input_text parts.
    Parts(Vec<InputText>),
}
/// Exactly one user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputMessage {
    /// Optional message tag.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<InputMessageKind>,
    /// User role.
    pub role: InputRole,
    /// Message text.
    pub content: InputContent,
}
/// Supported user input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    /// Plain text.
    Text(String),
    /// Exactly one user message.
    Messages([InputMessage; 1]),
}
impl ResponseInput {
    fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Messages([message]) => match &message.content {
                InputContent::Text(text) => text.is_empty(),
                InputContent::Parts(parts) => parts.iter().all(|part| part.text.is_empty()),
            },
        }
    }
}
impl From<&str> for ResponseInput {
    fn from(text: &str) -> Self {
        Self::Text(text.into())
    }
}
impl From<String> for ResponseInput {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}
/// Streaming durable response request; canonical create requires conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateResponseRequest {
    #[doc = "Optional final-answer admission condition, retained unchanged for recovery."]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_context: Option<crate::answer_context::AnswerContext>,
    /// Complete immutable event context; requires a current binding key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_context: Option<super::EventContext>,
    /// Existing Agent or binding conversation; optional on the per-Agent alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationReference>,
    /// One user message.
    pub input: ResponseInput,
    /// Must be true.
    pub stream: bool,
    /// Optional metadata, at most 16 string pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
    /// Non-null predecessors are unsupported in v2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    /// Must match the Agent's pinned model if supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Must be true if supplied.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
}
impl CreateResponseRequest {
    /// Construct a streaming request to an existing Agent.
    pub fn new(conversation: impl Into<String>, input: impl Into<ResponseInput>) -> Self {
        Self {
            answer_context: None,
            conversation: Some(ConversationReference::Id(conversation.into())),
            input: input.into(),
            stream: true,
            metadata: None,
            event_context: None,
            previous_response_id: None,
            model: None,
            store: None,
        }
    }
    pub(crate) fn validate(&self, alias: Option<&str>) -> Result<String> {
        if let Some(context) = &self.answer_context {
            context.validate()?;
        }
        let id = self
            .conversation
            .as_ref()
            .map(ConversationReference::id)
            .or(alias)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| Error::validation("conversation must select an existing Agent"))?;
        if let Some(context) = &self.event_context {
            context.validate()?;
        }
        if alias
            .is_some_and(|alias| alias != id && alias.starts_with("agt_") && id.starts_with("agt_"))
            || self.previous_response_id.is_some()
            || self
                .model
                .as_ref()
                .is_some_and(|model| model.trim().is_empty())
            || !self.stream
            || self.store == Some(false)
            || self.input.is_empty()
            || self.metadata.as_ref().is_some_and(|metadata| {
                metadata.len() > 16
                    || metadata
                        .iter()
                        .any(|(key, value)| key.chars().count() > 64 || value.chars().count() > 512)
            })
        {
            return Err(Error::validation(
                "Responses requires matching conversation, bounded text and metadata, streaming and durable storage without a predecessor",
            ));
        }
        Ok(id.into())
    }
}
