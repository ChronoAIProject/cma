//! Closed forms of the supported single user input.

use serde::{Deserialize, Serialize};

string_enum! { /// Explicit input item discriminant.
    pub enum InputMessageKind { Message => "message" }
}
string_enum! { /// The only supported response input role.
    pub enum InputRole { User => "user" }
}
string_enum! { /// The only supported input content part.
    pub enum InputTextKind { InputText => "input_text" }
}

/// One standard input text part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputText {
    /// Standard text-part discriminant.
    #[serde(rename = "type")]
    pub kind: InputTextKind,
    /// Nonempty user-authored text.
    pub text: String,
}

/// A single message may carry plain text or exactly one standard text part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputContent {
    /// Nonempty plain text.
    Text(String),
    /// Exactly one input text part.
    Parts([InputText; 1]),
}

/// Exactly one explicit user message for an existing conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputMessage {
    /// Optional standard message discriminant.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<InputMessageKind>,
    /// The supported user role.
    pub role: InputRole,
    /// Nonempty message text.
    pub content: InputContent,
}

/// Supported Responses input with the backend's exact one-message boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    /// Plain user text.
    Text(String),
    /// Exactly one explicit standard user message.
    Messages([InputMessage; 1]),
}

impl ResponseInput {
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Messages([message]) => match &message.content {
                InputContent::Text(text) => text.is_empty(),
                InputContent::Parts([part]) => part.text.is_empty(),
            },
        }
    }
}

impl From<String> for ResponseInput {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for ResponseInput {
    fn from(text: &str) -> Self {
        Self::Text(text.into())
    }
}
