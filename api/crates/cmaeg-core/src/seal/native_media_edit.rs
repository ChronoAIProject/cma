//! Revisioned single-upload authority. Historical effects-only references never gain it.
use super::MessageRefPayload;
use crate::{channel::valid_discord_id, ids::RecordId, message::NativeMediaKind, ValidationError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
pub const MAX_AGE: i64 = 86_400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "platform", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeMediaDestination {
    Telegram {
        bot_id: String,
        chat_id: String,
        thread_id: Option<String>,
    },
    Discord {
        bot_id: String,
        channel_id: String,
    },
}
impl NativeMediaDestination {
    pub fn bot(&self) -> &str {
        match self {
            Self::Telegram { bot_id, .. } | Self::Discord { bot_id, .. } => bot_id,
        }
    }
    pub fn channel(&self) -> &str {
        match self {
            Self::Telegram { chat_id, .. } => chat_id,
            Self::Discord { channel_id, .. } => channel_id,
        }
    }
    pub fn route(&self) -> String {
        let platform = match self {
            Self::Telegram { .. } => "telegram",
            Self::Discord { .. } => "discord",
        };
        format!("{platform}:{}:{}", self.bot(), self.channel())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeMediaEditAuthority {
    pub destination: NativeMediaDestination,
    pub record_id: RecordId,
    pub original_sha256: String,
    pub authority_sha256: String,
    pub kind: NativeMediaKind,
    pub revision: u64,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl NativeMediaEditAuthority {
    pub fn validate(&self, message: &MessageRefPayload) -> Result<(), ValidationError> {
        let invalid = || {
            ValidationError::new(
                "/native_media_edit",
                "exact confirmed single-media authority required",
            )
        };
        let hex = |v: &str| {
            v.len() == 64
                && v.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        let positive = |v: &str| v.parse::<i64>().is_ok_and(|n| n > 0 && n.to_string() == v);
        if message.received_reaction.is_some()
            || message.native_lark_effect.is_some()
            || message.whatsapp_reaction.is_some()
            || message.lark_effect.is_some()
            || message.relay_message.is_some()
            || message.interaction_record_id.is_some()
            || message.route_id != self.destination.route()
            || self.kind == NativeMediaKind::Sticker
            || !hex(&self.original_sha256)
            || !hex(&self.authority_sha256)
            || self.issued_at <= 0
            || self.expires_at <= self.issued_at
            || self.expires_at > self.issued_at.saturating_add(MAX_AGE)
        {
            return Err(invalid());
        }
        match &self.destination {
            NativeMediaDestination::Telegram {
                bot_id,
                chat_id,
                thread_id,
            } => {
                if !positive(bot_id)
                    || !positive(&message.platform_message_id)
                    || !chat_id
                        .parse::<i64>()
                        .is_ok_and(|n| n != 0 && n.to_string() == *chat_id)
                    || thread_id.as_deref().is_some_and(|s| !positive(s))
                    || message.discord_message.is_some()
                    || message.telegram_effect.as_ref().is_some_and(|a| {
                        !a.effects_only
                            || a.bot_id != *bot_id
                            || a.chat_id != *chat_id
                            || a.thread_id != *thread_id
                            || a.issued_at != self.issued_at
                            || a.expires_at != self.expires_at
                    })
                {
                    return Err(invalid());
                }
            }
            NativeMediaDestination::Discord { bot_id, channel_id } => {
                if !valid_discord_id(bot_id)
                    || !valid_discord_id(channel_id)
                    || !valid_discord_id(&message.platform_message_id)
                    || message.telegram_effect.is_some()
                    || message.discord_message.as_ref().is_some_and(|a| {
                        !a.effects_only
                            || a.bot_id != *bot_id
                            || a.channel_id != *channel_id
                            || a.effects.as_ref().is_none_or(|w| {
                                w.issued_at != self.issued_at || w.expires_at != self.expires_at
                            })
                    })
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
