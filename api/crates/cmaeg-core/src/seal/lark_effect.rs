//! Exact confirmed Lark/Feishu destination and its original custody.
use crate::{
    channel::{lark_resources::safe_id, LarkMessageEffectsBinding, Platform},
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const LARK_EFFECT_TTL: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LarkEffectAuthority {
    pub platform: Platform,
    pub binding: LarkMessageEffectsBinding,
    pub bot_id: String,
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub outbound_message_id: String,
    pub authority_sha256: String,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl LarkEffectAuthority {
    pub fn validate(&self, message: &super::MessageRefPayload) -> Result<(), ValidationError> {
        self.binding.validate("/lark_effect/binding")?;
        if !matches!(self.platform, Platform::Lark | Platform::Feishu)
            || !self.bot_id.starts_with("ou_")
            || !safe_id(&self.bot_id, 128)
            || !self.chat_id.starts_with("oc_")
            || !safe_id(&self.chat_id, 128)
            || !message.platform_message_id.starts_with("om_")
            || !safe_id(&message.platform_message_id, 128)
            || self.thread_id.as_ref().is_some_and(|id| !safe_id(id, 128))
            || self.outbound_message_id.is_empty()
            || self.outbound_message_id.len() > 512
            || self.outbound_message_id.chars().any(char::is_control)
            || self.authority_sha256.len() != 64
            || !self.authority_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.issued_at <= 0
            || self.expires_at <= self.issued_at
            || self.expires_at > self.issued_at.saturating_add(LARK_EFFECT_TTL)
            || message.native_media_edit.is_some()
            || message.received_reaction.is_some()
            || message.native_lark_effect.is_some()
            || message.discord_message.is_some()
            || message.telegram_effect.is_some()
            || message.relay_message.as_ref().is_some_and(|d| {
                d.platform != self.platform || d.outbound_message_id != self.outbound_message_id
            })
        {
            return Err(ValidationError::new(
                "/lark_effect",
                "invalid confirmed Lark message authority",
            ));
        }
        Ok(())
    }
}
