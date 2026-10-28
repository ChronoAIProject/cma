//! Continuation is bound to an independently supplied original event, never a new destination.
use super::{open, seal, MasterKeyRing, OpenError, Opened, SealError, TokenKind};
use crate::{channel::valid_discord_id, conversation_history::HISTORY_PAGE_SIZE, ValidationError};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryOrigin {
    pub channel_id: String,
    pub channel_type: u8,
    pub guild_id: Option<String>,
    pub parent_id: Option<String>,
}
impl HistoryOrigin {
    pub fn valid(&self) -> bool {
        valid_discord_id(&self.channel_id)
            && self.guild_id.as_deref().is_none_or(valid_discord_id)
            && self
                .parent_id
                .as_deref()
                .is_none_or(|id| valid_discord_id(id) && id != self.channel_id)
            && match self.channel_type {
                1 => self.guild_id.is_none() && self.parent_id.is_none(),
                0 | 2 | 5 | 13 => self.guild_id.is_some() && self.parent_id.is_none(),
                10..=12 => self.guild_id.is_some() && self.parent_id.is_some(),
                _ => false,
            }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryCursorPayload {
    pub v: u8,
    pub event_sha256: String,
    pub custody: String,
    pub origin: HistoryOrigin,
    pub anchor_message_id: String,
    pub before_message_id: String,
    pub page_size: u8,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl HistoryCursorPayload {
    pub fn valid(&self) -> bool {
        let hash = |value: &str| value.len() == 64 && value.bytes().all(|v| v.is_ascii_hexdigit());
        self.v == 1
            && hash(&self.event_sha256)
            && hash(&self.custody)
            && self.origin.valid()
            && self.page_size == HISTORY_PAGE_SIZE
            && valid_discord_id(&self.anchor_message_id)
            && valid_discord_id(&self.before_message_id)
            && matches!(
                (self.before_message_id.parse::<u64>(), self.anchor_message_id.parse::<u64>()),
                (Ok(before), Ok(anchor)) if before > 0 && before < anchor
            )
            && self.issued_at > 0
            && self.issued_at < self.expires_at
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct HistoryCursor(String);
impl fmt::Debug for HistoryCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted history cursor]")
    }
}
impl FromStr for HistoryCursor {
    type Err = ValidationError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if !raw.starts_with("cmaeg1.hc.") || raw.len() > super::MAX_TOKEN_LENGTH {
            return Err(ValidationError::new("/", "history cursor is invalid"));
        }
        Ok(Self(raw.into()))
    }
}
impl HistoryCursor {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn seal(payload: &HistoryCursorPayload, keys: &MasterKeyRing) -> Result<Self, SealError> {
        if !payload.valid() {
            return Err(SealError::InvalidPayload);
        }
        seal(keys, TokenKind::HistoryCursor, payload).map(Self)
    }
    pub fn open(
        &self,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<HistoryCursorPayload>, OpenError> {
        let opened: Opened<HistoryCursorPayload> = open(keys, TokenKind::HistoryCursor, &self.0)?;
        if !opened.payload.valid() || opened.payload.issued_at > now {
            return Err(OpenError::Format);
        }
        if now >= opened.payload.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}

/// Provider pagination is opaque; message IDs never order native Lark rows.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLarkHistoryCursorPayload {
    pub v: u8,
    pub event_sha256: String,
    pub custody: String,
    pub origin: crate::channel::NativeLarkReply,
    pub anchor_created_ms: i64,
    /// Last scanned pre-cutoff ID, or the original anchor while seeking past newer rows.
    pub before_message_id: String,
    pub page_token: String,
    pub last_scanned_ms: Option<i64>,
    pub last_scanned_id: Option<String>,
    pub page_size: u8,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl NativeLarkHistoryCursorPayload {
    pub fn valid(&self) -> bool {
        let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
        let id = |s: &str| crate::channel::native_lark::safe_id(s) && s.starts_with("om_");
        self.v == 2
            && hash(&self.event_sha256)
            && hash(&self.custody)
            && self.origin.validate().is_ok()
            && self
                .origin
                .thread_id
                .as_deref()
                .is_none_or(|s| s.starts_with("omt_"))
            && self.anchor_created_ms > 0
            && id(&self.before_message_id)
            && self.page_size == HISTORY_PAGE_SIZE
            && !self.page_token.is_empty()
            && self.page_token.len() <= 1024
            && !self.page_token.chars().any(char::is_control)
            && self.last_scanned_ms.is_none_or(|ms| ms > 0)
            && self.last_scanned_id.as_deref().is_none_or(id)
            && (self.last_scanned_ms.is_some() == self.last_scanned_id.is_some())
            && self.issued_at > 0
            && self.issued_at < self.expires_at
    }
}
impl HistoryCursor {
    pub fn seal_native_lark(
        payload: &NativeLarkHistoryCursorPayload,
        keys: &MasterKeyRing,
    ) -> Result<Self, SealError> {
        if !payload.valid() {
            return Err(SealError::InvalidPayload);
        }
        seal(keys, TokenKind::HistoryCursor, payload).map(Self)
    }
    pub fn open_native_lark(
        &self,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<NativeLarkHistoryCursorPayload>, OpenError> {
        let opened: Opened<NativeLarkHistoryCursorPayload> =
            open(keys, TokenKind::HistoryCursor, &self.0)?;
        if !opened.payload.valid() || opened.payload.issued_at > now {
            return Err(OpenError::Format);
        }
        if now >= opened.payload.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}
