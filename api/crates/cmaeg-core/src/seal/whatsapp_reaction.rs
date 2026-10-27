//! Reaction-only authority for one authenticated received WhatsApp message.
use crate::{
    channel::whatsapp::{valid_message_id, valid_meta_id},
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_REACTION_AGE: i64 = 30 * 86400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WhatsappReactionAuthority {
    pub app_id: String,
    pub waba_id: String,
    pub phone_number_id: String,
    pub recipient_id: String,
    pub authority_sha256: String,
    pub received_at: i64,
    pub expires_at: i64,
}
impl WhatsappReactionAuthority {
    pub fn route(&self) -> String {
        format!(
            "whatsapp:{}:{}:{}:{}",
            self.app_id, self.waba_id, self.phone_number_id, self.recipient_id
        )
    }
    pub fn validate(&self, message: &super::MessageRefPayload) -> Result<(), ValidationError> {
        if [
            &self.app_id,
            &self.waba_id,
            &self.phone_number_id,
            &self.recipient_id,
        ]
        .iter()
        .any(|id| !valid_meta_id(id))
            || !valid_message_id(&message.platform_message_id)
            || message.route_id != self.route()
            || self.authority_sha256.len() != 64
            || !self.authority_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.received_at <= 0
            || self.expires_at <= self.received_at
            || self.expires_at > self.received_at.saturating_add(MAX_REACTION_AGE)
            || message.received_reaction.is_some()
            || message.native_lark_effect.is_some()
            || message.discord_message.is_some()
            || message.telegram_effect.is_some()
            || message.lark_effect.is_some()
            || message.interaction_record_id.is_some()
            || message.relay_message.is_some()
        {
            return Err(ValidationError::new(
                "/whatsapp_reaction",
                "invalid received WhatsApp reaction authority",
            ));
        }
        Ok(())
    }
}
