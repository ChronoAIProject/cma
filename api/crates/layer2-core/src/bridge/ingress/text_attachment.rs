//! A bounded text file becomes ordinary untrusted Agent text after capture.
//! The CDN credential is separate expiring custody, never retained history.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{CallbackError, StoredInbox};

pub const MAX_CHANNEL_TEXT_ATTACHMENT_BYTES: usize = 16_384;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordTextAttachment {
    pub attachment_id: String,
    pub filename: String,
    pub size_bytes: u64,
    pub source_url_sha256: String,
    pub source_expires_at: i64,
    pub custody_expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured: Option<CapturedAttachmentText>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedAttachmentText {
    pub text: String,
    pub sha256: String,
}

impl DiscordTextAttachment {
    pub fn validate(&self, inbox: &StoredInbox) -> Result<(), CallbackError> {
        if inbox.envelope.platform != "discord"
            || !inbox.envelope.attachments.is_empty()
            || inbox.envelope.content_type != "text"
            || inbox.envelope.text.as_ref().is_none_or(|s| s.len() > 8192)
            || !snowflake(&self.attachment_id)
            || self.filename.is_empty()
            || self.filename.len() > 128
            || !self.filename.ends_with(".txt")
            || !self
                .filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || self.filename.starts_with('.')
            || self.size_bytes == 0
            || self.size_bytes > MAX_CHANNEL_TEXT_ATTACHMENT_BYTES as u64
            || self.source_url_sha256.len() != 64
            || !self
                .source_url_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.custody_expires_at <= inbox.accepted_at
            || self.custody_expires_at > inbox.accepted_at.saturating_add(900)
            || self.custody_expires_at > inbox.expires_at
            || self.custody_expires_at > self.source_expires_at
        {
            return Err(CallbackError::Payload);
        }
        if let Some(captured) = &self.captured {
            if !valid_text(captured.text.as_bytes())
                || captured.text.len() as u64 != self.size_bytes
                || captured.sha256 != format!("{:x}", Sha256::digest(captured.text.as_bytes()))
            {
                return Err(CallbackError::Payload);
            }
        }
        Ok(())
    }

    /// Expiry governs pending file custody and new command admission. Text in
    /// an already admitted command follows the Agent's ordinary history policy.
    pub fn require_custody(&self, inbox: &StoredInbox, now: i64) -> Result<(), CallbackError> {
        self.validate(inbox)?;
        if self.custody_expires_at <= now || inbox.expires_at <= now {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }

    pub fn agent_text(&self, inbox: &StoredInbox) -> Result<String, CallbackError> {
        self.validate(inbox)?;
        let captured = self.captured.as_ref().ok_or(CallbackError::Unavailable)?;
        serde_json::to_string(&serde_json::json!({
            "kind": "channel_user_text_attachment_v1",
            "caption": inbox.envelope.text,
            "attachment": {"filename": self.filename, "media_type": "text/plain",
                "size_bytes": self.size_bytes, "sha256": captured.sha256,
                "untrusted_text": captured.text}
        }))
        .map_err(|_| CallbackError::Payload)
    }
}

pub fn valid_text(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.len() <= MAX_CHANNEL_TEXT_ATTACHMENT_BYTES
        && std::str::from_utf8(bytes).is_ok_and(|text| {
            text.chars()
                .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        })
}

pub fn snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && !value.starts_with('0')
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

/// Only the trusted connector extractor supplies this after checking exact
/// signed raw platform fields. A caller-provided URL is never sufficient.
pub struct VerifiedTextAttachment {
    pub(super) metadata: DiscordTextAttachment,
    pub(super) url: Zeroizing<String>,
}

impl VerifiedTextAttachment {
    pub fn metadata(&self) -> &DiscordTextAttachment {
        &self.metadata
    }
    pub fn from_connector(
        inbox: &StoredInbox,
        metadata: DiscordTextAttachment,
        url: String,
    ) -> Result<Self, CallbackError> {
        metadata.validate(inbox)?;
        if metadata.captured.is_some()
            || url.len() > 4096
            || metadata.source_url_sha256 != format!("{:x}", Sha256::digest(url.as_bytes()))
        {
            return Err(CallbackError::Payload);
        }
        Ok(Self {
            metadata,
            url: Zeroizing::new(url),
        })
    }
}
