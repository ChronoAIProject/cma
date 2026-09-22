use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Complete event snapshot admitted atomically with a binding turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventContext {
    /// Must be one.
    pub schema_version: u16,
    /// External channel identity, distinct from the provider binding.
    pub channel_id: String,
    /// External conversation identity, even when the turn uses an Agent alias.
    pub conversation_id: String,
    /// Full activity, including its schema version and event identity.
    pub activity: serde_json::Map<String, serde_json::Value>,
    /// Opaque reference exposed to the Agent.
    pub event_ref: String,
}
impl EventContext {
    pub(crate) fn validate(&self) -> Result<()> {
        let label = |value: &str, limit| {
            !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
        };
        if self.schema_version != 1
            || self.activity.get("schema_version") != Some(&serde_json::json!(1))
            || !self
                .activity
                .get("event_id")
                .and_then(|value| value.as_str())
                .is_some_and(|id| label(id, 512))
            || !label(&self.channel_id, 512)
            || !label(&self.conversation_id, 512)
            || !label(&self.event_ref, 4096)
            || serde_json::to_vec(self).map_or(true, |bytes| bytes.len() > 1_048_576)
        {
            return Err(Error::validation(
                "event context is invalid or exceeds 1 MiB",
            ));
        }
        Ok(())
    }
}
impl super::ResponsesV2<'_> {
    /// Read one complete event using its owning binding key; no automatic retry.
    pub async fn read_event_context(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<EventContext> {
        let context: EventContext = self
            .get(
                &[
                    "api",
                    "v1",
                    "cmaeg",
                    "bindings",
                    binding,
                    "conversations",
                    conversation,
                    "events",
                    event,
                ],
                &[],
            )
            .await?;
        context
            .validate()
            .map_err(|_| Error::protocol("invalid event context response"))?;
        if context.conversation_id != conversation
            || context
                .activity
                .get("event_id")
                .and_then(|value| value.as_str())
                != Some(event)
        {
            return Err(Error::protocol("event context response identity mismatch"));
        }
        Ok(context)
    }
}
