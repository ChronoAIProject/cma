//! A thread derived from one admitted human event; never arbitrary channel authority.
use super::{
    open, seal, EventRefPayload, MasterKeyRing, OpenError, Opened, ReplyAddress, SealError,
    TokenKind,
};
use crate::{channel::valid_discord_id, ValidationError};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use utoipa::ToSchema;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadRefPayload {
    pub v: u8,
    pub event: EventRefPayload,
    /// Hash of exact source, owner, selected consumer/provider/key/credential and bot token.
    pub custody: String,
    pub guild_id: String,
    pub parent_id: String,
    pub thread_id: String,
    pub thread_type: u8,
    pub issued_at: i64,
    /// Always equal to the original event expiry, including list-derived references.
    pub expires_at: i64,
}
impl ThreadRefPayload {
    pub fn valid(&self) -> bool {
        let ReplyAddress::Discord {
            channel_id,
            thread_id,
            ..
        } = &self.event.reply
        else {
            return false;
        };
        self.v == 1
            && self.event.validate().is_ok()
            && self.event.activity_sha256.is_some()
            && self.event.lineage.is_none()
            && self.custody.len() == 64
            && self.custody.bytes().all(|b| b.is_ascii_hexdigit())
            && [&self.guild_id, &self.parent_id, &self.thread_id]
                .iter()
                .all(|id| valid_discord_id(id))
            && self.parent_id != self.thread_id
            && matches!(self.thread_type, 10..=12)
            && self.issued_at > 0
            && self.issued_at < self.expires_at
            && self.expires_at == self.event.expires_at
            && match thread_id {
                Some(original) => original == channel_id && original == &self.thread_id,
                None => channel_id == &self.parent_id && matches!(self.thread_type, 10 | 11),
            }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct ThreadRef(String);
impl fmt::Debug for ThreadRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted thread reference]")
    }
}
impl FromStr for ThreadRef {
    type Err = ValidationError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if !raw.starts_with("cmaeg1.tr.") || raw.len() > super::MAX_TOKEN_LENGTH {
            return Err(ValidationError::new("/", "thread reference is invalid"));
        }
        Ok(Self(raw.into()))
    }
}
impl ThreadRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn seal(payload: &ThreadRefPayload, keys: &MasterKeyRing) -> Result<Self, SealError> {
        if !payload.valid() {
            return Err(SealError::InvalidPayload);
        }
        seal(keys, TokenKind::ThreadRef, payload).map(Self)
    }
    pub fn open(
        &self,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<ThreadRefPayload>, OpenError> {
        let opened: Opened<ThreadRefPayload> = open(keys, TokenKind::ThreadRef, &self.0)?;
        if !opened.payload.valid() || opened.payload.issued_at > now {
            return Err(OpenError::Format);
        }
        if now >= opened.payload.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}
