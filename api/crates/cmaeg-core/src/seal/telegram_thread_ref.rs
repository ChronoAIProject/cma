//! A confirmed created Telegram topic, scoped to the original admitted human event.
use super::{
    open, seal, EventRefPayload, MasterKeyRing, OpenError, Opened, ReplyAddress, SealError,
    ThreadRef, TokenKind,
};
use serde::{Deserialize, Serialize};

/// Separate v2 payload; existing Discord v1 references and their bytes stay unchanged.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramThreadRefPayload {
    pub v: u8,
    pub event: EventRefPayload,
    pub custody: String,
    pub chat_id: String,
    pub thread_id: String,
    pub issued_at: i64,
    pub expires_at: i64,
}
impl TelegramThreadRefPayload {
    pub fn valid(&self) -> bool {
        let ReplyAddress::Telegram { chat_id, .. } = &self.event.reply else {
            return false;
        };
        self.v == 2
            && self.event.validate().is_ok()
            && self.event.activity_sha256.is_some()
            && self.event.lineage.is_none()
            && chat_id == &self.chat_id
            && self
                .chat_id
                .parse::<i64>()
                .is_ok_and(|id| id < 0 && id.to_string() == self.chat_id)
            && self
                .thread_id
                .parse::<i64>()
                .is_ok_and(|id| id > 1 && id.to_string() == self.thread_id)
            && self.custody.len() == 64
            && self.custody.bytes().all(|b| b.is_ascii_hexdigit())
            && self.issued_at > 0
            && self.issued_at < self.expires_at
            && self.expires_at == self.event.expires_at
    }
}
impl ThreadRef {
    pub fn seal_telegram(
        payload: &TelegramThreadRefPayload,
        keys: &MasterKeyRing,
    ) -> Result<Self, SealError> {
        if !payload.valid() {
            return Err(SealError::InvalidPayload);
        }
        seal(keys, TokenKind::ThreadRef, payload)?
            .parse()
            .map_err(|_| SealError::InvalidPayload)
    }
    pub fn open_telegram(
        &self,
        keys: &MasterKeyRing,
        now: i64,
    ) -> Result<Opened<TelegramThreadRefPayload>, OpenError> {
        let opened: Opened<TelegramThreadRefPayload> =
            open(keys, TokenKind::ThreadRef, self.as_str())?;
        if !opened.payload.valid() || opened.payload.issued_at > now {
            return Err(OpenError::Format);
        }
        if now >= opened.payload.expires_at {
            return Err(OpenError::Expired);
        }
        Ok(opened)
    }
}
