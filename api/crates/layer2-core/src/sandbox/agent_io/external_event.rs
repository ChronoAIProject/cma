use super::{AgentCommand, AgentCommandEnvelope, AgentContent, AgentInput};
use crate::sandbox::AgentOperationError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_EXTERNAL_EVENT_BYTES: usize = 1_048_576;

/// Immutable external event data admitted with an input. The interface owns the
/// payload schema; the core retains every field without projecting or fetching it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalEventContext {
    /// Selected by the provider adapter after authenticating the source event.
    /// The original transport conversation remains immutable in `payload`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_conversation_id: Option<String>,
    pub source_id: String,
    pub conversation_id: String,
    pub event_id: String,
    pub reference: String,
    pub payload: serde_json::Value,
}
impl ExternalEventContext {
    pub fn validate(&self) -> Result<(), AgentOperationError> {
        let valid = |value: &str, limit| {
            !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
        };
        if !valid(&self.source_id, 512)
            || self
                .provider_conversation_id
                .as_deref()
                .is_some_and(|id| !valid(id, 512))
            || !valid(&self.conversation_id, 512)
            || !valid(&self.event_id, 512)
            || !valid(&self.reference, 4096)
            || !self.payload.is_object()
            || serde_json::to_vec(&self.payload)
                .map_or(true, |bytes| bytes.len() > MAX_EXTERNAL_EVENT_BYTES)
        {
            return Err(super::input::invalid("external event context is invalid"));
        }
        Ok(())
    }
    pub(super) fn runtime_reference(&self) -> String {
        format!("\n\nevent_ref: {}", self.reference)
    }
}
impl AgentInput {
    pub fn external_event(&self) -> Option<&ExternalEventContext> {
        self.content.iter().find_map(|item| match item {
            AgentContent::ExternalEvent { context } => Some(context),
            _ => None,
        })
    }
}
impl AgentCommand {
    pub fn external_event_mut(&mut self) -> Option<&mut ExternalEventContext> {
        let input = match self {
            Self::Message { input, .. } | Self::Steer { input } => input,
            _ => return None,
        };
        input.content.iter_mut().find_map(|item| match item {
            AgentContent::ExternalEvent { context } => Some(context),
            _ => None,
        })
    }
    pub fn external_event(&self) -> Option<&ExternalEventContext> {
        match self {
            Self::Message { input, .. } | Self::Steer { input } => input.external_event(),
            _ => None,
        }
    }
}
impl AgentCommandEnvelope {
    /// Permanent event identity is independent of key rotation and admission keys.
    pub fn external_event_key(&self) -> Option<String> {
        self.command.external_event().map(|context| {
            external_event_key(
                &self.authorization.authority_id,
                &context.conversation_id,
                &context.event_id,
            )
        })
    }
}
pub fn external_event_key(authority: &str, conversation: &str, event: &str) -> String {
    let bytes = serde_json::to_vec(&(authority, conversation, event))
        .expect("string tuple is serializable");
    format!("{:x}", Sha256::digest(bytes))
}
