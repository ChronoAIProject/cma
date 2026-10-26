//! A confirmed destination, authenticated under its own token kind.
use crate::{
    channel::Platform,
    ids::{ChannelId, ConsumerId, EventId},
    require_string, ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageRefPayload {
    /// Optional authority for new native Telegram delete/reaction tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_effect: Option<super::TelegramEffectAuthority>,
    pub channel_id: ChannelId,
    pub event_id: EventId,
    pub platform_message_id: String,
    pub route_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_record_id: Option<String>,
    /// Exact relay receipt for an ordinary, single-part text message. Legacy
    /// references omit this and retain their original update behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_message: Option<RelayMessageDestination>,
    /// The consumer this reference authorizes; absent for the default
    /// consumer (never `Some("default")`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer: Option<ConsumerId>,
}
impl MessageRefPayload {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Some(effect) = &self.telegram_effect {
            effect.validate()?;
            if self.relay_message.is_some()
                || self.route_id != format!("telegram:{}:{}", effect.bot_id, effect.chat_id)
                || !self
                    .platform_message_id
                    .parse::<i64>()
                    .is_ok_and(|n| n > 0 && n.to_string() == self.platform_message_id)
            {
                return Err(ValidationError::new(
                    "/telegram_effect",
                    "native confirmed message required",
                ));
            }
        }
        require_string(&self.platform_message_id, "/platform_message_id", 1, 512)?;
        require_string(&self.route_id, "/route_id", 1, 512)?;
        if let Some(destination) = &self.relay_message {
            require_string(
                &destination.outbound_message_id,
                "/relay_message/outbound_message_id",
                1,
                512,
            )?;
            if destination
                .outbound_message_id
                .chars()
                .any(char::is_control)
                || self.interaction_record_id.is_some()
                || !matches!(
                    destination.platform,
                    Platform::Telegram | Platform::Lark | Platform::Feishu | Platform::Discord
                )
            {
                return Err(ValidationError::new(
                    "/relay_message",
                    "invalid relay edit destination",
                ));
            }
        }
        super::event_ref::validate_consumer(self.consumer.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RelayMessageDestination {
    pub platform: Platform,
    pub outbound_message_id: String,
}
