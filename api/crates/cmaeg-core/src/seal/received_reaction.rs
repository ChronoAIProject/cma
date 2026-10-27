//! Reaction-only scope for one retained, authenticated incoming user message.
use super::MessageRefPayload;
use crate::{channel::valid_discord_id, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Fixed lifetime from the original message, never renewed by issuing a new seal.
pub const MAX_AGE: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceivedReactionPlatform {
    Telegram,
    Discord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReceivedReactionAuthority {
    pub platform: ReceivedReactionPlatform,
    pub bot_id: String,
    pub chat_id: String,
    pub actor_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Exact original event, including provider conversation and partition.
    pub event_sha256: String,
    /// Current source, credentials, owner and selected consumer/provider revision.
    pub authority_sha256: String,
    pub received_at: i64,
    pub expires_at: i64,
}
impl ReceivedReactionAuthority {
    pub fn route(&self) -> String {
        let platform = match self.platform {
            ReceivedReactionPlatform::Telegram => "telegram",
            ReceivedReactionPlatform::Discord => "discord",
        };
        format!("received:{platform}:{}:{}", self.bot_id, self.chat_id)
    }

    pub fn validate(&self, message: &MessageRefPayload) -> Result<(), ValidationError> {
        let positive = |s: &str| s.parse::<i64>().is_ok_and(|n| n > 0 && n.to_string() == s);
        let ids_valid = match self.platform {
            ReceivedReactionPlatform::Telegram => {
                positive(&self.bot_id)
                    && positive(&self.actor_id)
                    && positive(&message.platform_message_id)
                    && self
                        .chat_id
                        .parse::<i64>()
                        .is_ok_and(|n| n != 0 && n.to_string() == self.chat_id)
                    && self.thread_id.as_deref().is_none_or(positive)
            }
            ReceivedReactionPlatform::Discord => {
                [
                    &self.bot_id,
                    &self.chat_id,
                    &self.actor_id,
                    &message.platform_message_id,
                ]
                .iter()
                .all(|id| valid_discord_id(id))
                    && self
                        .thread_id
                        .as_deref()
                        .is_none_or(|id| id == self.chat_id)
            }
        };
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !ids_valid
            || self.actor_id == self.bot_id
            || !digest(&self.event_sha256)
            || !digest(&self.authority_sha256)
            || self.received_at <= 0
            || self.expires_at <= self.received_at
            || self.expires_at > self.received_at.saturating_add(MAX_AGE)
            || message.route_id != self.route()
            || message.whatsapp_reaction.is_some()
            || message.discord_message.is_some()
            || message.telegram_effect.is_some()
            || message.lark_effect.is_some()
            || message.interaction_record_id.is_some()
            || message.relay_message.is_some()
        {
            return Err(ValidationError::new(
                "/received_reaction",
                "invalid received-message reaction authority",
            ));
        }
        Ok(())
    }
}
