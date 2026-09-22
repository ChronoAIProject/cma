use serde::{Deserialize, Serialize};

use super::super::{AgentOperationError, MAX_AGENT_MESSAGE_BYTES};

const MAX_CONTENT_ITEMS: usize = 32;

/// An immutable, separately authorized stored object; never a fetch URL or key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAttachmentReference {
    pub id: String,
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentContent {
    Text {
        text: String,
    },
    ExternalEvent {
        context: super::ExternalEventContext,
    },
    Attachment {
        reference: AgentAttachmentReference,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInput {
    pub content: Vec<AgentContent>,
}

impl AgentInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![AgentContent::Text { text: text.into() }],
        }
    }

    /// Merge adjacent text items without changing any whitespace or bytes.
    pub fn canonical(&self) -> Result<Self, AgentOperationError> {
        if self.content.is_empty() || self.content.len() > MAX_CONTENT_ITEMS {
            return Err(invalid("input content count is invalid"));
        }
        let mut content = Vec::new();
        let mut bytes = 0usize;
        let mut meaningful = false;
        let mut external = false;
        for item in &self.content {
            match item {
                AgentContent::Text { text } => {
                    bytes = bytes
                        .checked_add(text.len())
                        .ok_or_else(|| invalid("input is too large"))?;
                    if bytes > MAX_AGENT_MESSAGE_BYTES {
                        return Err(invalid("input is too large"));
                    }
                    meaningful |= !text.trim().is_empty();
                    if let Some(AgentContent::Text { text: previous }) = content.last_mut() {
                        previous.push_str(text);
                    } else {
                        content.push(item.clone());
                    }
                }
                AgentContent::ExternalEvent { context } => {
                    if external {
                        return Err(invalid("only one external event is allowed"));
                    }
                    external = true;
                    context.validate()?;
                    bytes += context.runtime_reference().len();
                    content.push(item.clone());
                }
                AgentContent::Attachment { reference } => {
                    if !valid_label(&reference.id, 256)
                        || !valid_label(&reference.media_type, 128)
                        || !reference.media_type.contains('/')
                        || reference.sha256.len() != 64
                        || !reference
                            .sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        || reference.size_bytes == 0
                    {
                        return Err(invalid("attachment reference is invalid"));
                    }
                    meaningful = true;
                    content.push(item.clone());
                }
            }
        }
        if bytes > MAX_AGENT_MESSAGE_BYTES {
            return Err(invalid("input is too large"));
        }
        if !meaningful {
            return Err(invalid("input must contain content"));
        }
        Ok(Self { content })
    }

    /// Current runtime dispatch accepts text. Refuse attachments explicitly;
    /// silently dropping them would change both the prompt and its fingerprint.
    pub(super) fn into_text(self) -> Result<String, AgentOperationError> {
        let canonical = self.canonical()?;
        let mut text = String::new();
        for item in canonical.content {
            match item {
                AgentContent::Text { text: part } => text.push_str(&part),
                AgentContent::ExternalEvent { context } => {
                    text.push_str(&context.runtime_reference())
                }
                AgentContent::Attachment { .. } => {
                    return Err(AgentOperationError::ContentUnsupported)
                }
            }
        }
        Ok(text)
    }
}

fn valid_label(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

pub(super) fn invalid(detail: &str) -> AgentOperationError {
    AgentOperationError::InvalidRequest(detail.into())
}
