//! Bounded delayed-reply authority; never a user identity or a routing policy.
use super::{open, seal, EventRefPayload, MasterKeyRing, OpenError, Opened, SealError, TokenKind};
use crate::{
    ids::{BindingId, ProviderSlug},
    ValidationError,
};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use utoipa::ToSchema;

pub const REPLY_TARGET_TTL: i64 = 24 * 60 * 60;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyTargetPayload {
    pub v: u8,
    pub provider: ProviderSlug,
    pub binding_id: BindingId,
    pub agent_key_hash: [u8; 32],
    pub issued_at: i64,
    pub expires_at: i64,
    pub event: EventRefPayload,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct ReplyTargetRef(String);

impl fmt::Debug for ReplyTargetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted reply target]")
    }
}
impl FromStr for ReplyTargetRef {
    type Err = ValidationError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if !raw.starts_with("cmaeg1.rt.") || raw.len() > super::MAX_TOKEN_LENGTH {
            return Err(ValidationError::new("/", "reply target is invalid"));
        }
        Ok(Self(raw.into()))
    }
}
impl ReplyTargetRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn seal(payload: &ReplyTargetPayload, keys: &MasterKeyRing) -> Result<Self, SealError> {
        seal(keys, TokenKind::ReplyTarget, payload).map(Self)
    }
    pub fn open(
        &self,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<ReplyTargetPayload>, OpenError> {
        let opened: Opened<ReplyTargetPayload> = open(keys, TokenKind::ReplyTarget, &self.0)?;
        let p = &opened.payload;
        if p.v != 1
            || p.issued_at <= 0
            || p.issued_at > now
            || p.expires_at <= p.issued_at
            || p.expires_at > p.issued_at.saturating_add(REPLY_TARGET_TTL)
            || p.issued_at >= p.event.expires_at
            || p.event.activity_sha256.is_none()
            || p.event.validate().is_err()
            || !matches!(
                p.event.reply,
                super::ReplyAddress::NyxidRelay { .. } | super::ReplyAddress::Telegram { .. }
            )
        {
            return Err(OpenError::Format);
        }
        if now >= p.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}
