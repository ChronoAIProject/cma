//! Separate-app authority for exactly one retained native Lark/Feishu message.
use crate::{
    channel::{native_lark::safe_id, NativeLarkReply},
    ids::RecordId,
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Fixed from the original receipt/message, never extended by resealing a reference.
pub const MAX_AGE: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeLarkEffectTarget {
    Confirmed { editable: bool },
    Received { sender_id: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkEffectAuthority {
    pub reply: NativeLarkReply,
    pub record_id: RecordId,
    pub target: NativeLarkEffectTarget,
    /// Exact persisted outbound receipt or admitted original event, including routing.
    pub original_sha256: String,
    pub authority_sha256: String,
    pub observed_at: i64,
    pub expires_at: i64,
}
impl NativeLarkEffectAuthority {
    pub fn route(&self) -> String {
        format!(
            "native-lark:{}:{}:{}",
            self.reply.identity.region.as_str(),
            self.reply.identity.app_id,
            self.reply.chat_id
        )
    }
    pub fn confirmed(&self) -> bool {
        matches!(self.target, NativeLarkEffectTarget::Confirmed { .. })
    }
    pub fn editable(&self) -> bool {
        matches!(
            self.target,
            NativeLarkEffectTarget::Confirmed { editable: true }
        )
    }
    pub fn validate(&self, message: &super::MessageRefPayload) -> Result<(), ValidationError> {
        self.reply.validate()?;
        let digest = |s: &str| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        if !digest(&self.original_sha256)
            || !digest(&self.authority_sha256)
            || self.observed_at <= 0
            || self.expires_at <= self.observed_at
            || self.expires_at > self.observed_at.saturating_add(MAX_AGE)
            || message.platform_message_id != self.reply.message_id
            || message.route_id != self.route()
            || message.whatsapp_reaction.is_some()
            || message.discord_message.is_some()
            || message.telegram_effect.is_some()
            || message.lark_effect.is_some()
            || message.relay_message.is_some()
            || message.interaction_record_id.is_some()
            || matches!(&self.target, NativeLarkEffectTarget::Received { sender_id }
                if !safe_id(sender_id) || !sender_id.starts_with("ou_") || *sender_id == self.reply.identity.bot_open_id)
        {
            return Err(ValidationError::new(
                "/native_lark_effect",
                "invalid native Lark message authority",
            ));
        }
        Ok(())
    }
}
