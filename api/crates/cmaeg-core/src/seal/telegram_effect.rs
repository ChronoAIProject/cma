//! Additive authority for effects on confirmed native Telegram bot messages.
use crate::{
    activity::{ActorKind, ConversationKind},
    channel::ProviderBinding,
    ids::RecordId,
    profile::NyxIdIdentity,
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const TELEGRAM_EFFECT_TTL: i64 = 24 * 60 * 60;

/// Facts from the authenticated native update, retained before normalization
/// discards reply-to-bot evidence. These are never accepted from a tool caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TelegramAdmissionProof {
    pub actor_id: String,
    pub actor_kind: ActorKind,
    pub conversation_kind: ConversationKind,
    pub mentioned: Option<bool>,
    pub replied_to_bot: Option<bool>,
}
impl TelegramAdmissionProof {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !self
            .actor_id
            .parse::<i64>()
            .is_ok_and(|id| id > 0 && id.to_string() == self.actor_id)
            || !matches!(self.actor_kind, ActorKind::Human | ActorKind::Bot)
            || self.conversation_kind == ConversationKind::Custom
        {
            return Err(ValidationError::new(
                "/telegram_admission",
                "invalid native admission proof",
            ));
        }
        Ok(())
    }
}

/// Bound to the containing MessageRef's channel, consumer, event and exact
/// platform message. Missing on retained references and all relay messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TelegramEffectAuthority {
    pub record_id: RecordId,
    pub owner: NyxIdIdentity,
    pub provider: ProviderBinding,
    pub agent_key_hash: [u8; 32],
    pub routing_sha256: String,
    pub bot_id: String,
    pub chat_id: String,
    pub thread_id: Option<String>,
    pub admission: TelegramAdmissionProof,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl TelegramEffectAuthority {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.admission.validate()?;
        if self.routing_sha256.len() != 64
            || !self.routing_sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ValidationError::new(
                "/telegram_effect",
                "invalid routing authority",
            ));
        }
        for (id, positive) in [(&self.bot_id, true), (&self.chat_id, false)] {
            if !id
                .parse::<i64>()
                .is_ok_and(|n| (if positive { n > 0 } else { n != 0 }) && n.to_string() == *id)
            {
                return Err(ValidationError::new(
                    "/telegram_effect",
                    "invalid native destination",
                ));
            }
        }
        if self.issued_at <= 0
            || self.expires_at <= self.issued_at
            || self.expires_at > self.issued_at.saturating_add(TELEGRAM_EFFECT_TTL)
            || self.thread_id.as_ref().is_some_and(|id| {
                !id.parse::<i64>()
                    .is_ok_and(|n| n > 0 && n.to_string() == *id)
            })
        {
            return Err(ValidationError::new(
                "/telegram_effect",
                "invalid native effect authority",
            ));
        }
        Ok(())
    }
}
