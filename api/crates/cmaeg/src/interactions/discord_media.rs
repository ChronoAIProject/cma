//! A compact upload fact on the same durable card; source URLs/bytes are never retained.
use super::Error;
use crate::media::outbound::{PreparedFile, MAX_OUTBOUND_BYTES};
use cmaeg_core::message::{NativeAttachment, NativeMediaKind, NativeMediaSource, OutboundMessage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    kind: NativeMediaKind,
    sha256: String,
    bytes: u64,
    filename: String,
    mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caption: Option<String>,
}
impl Media {
    pub(super) fn prepare(message: &OutboundMessage, file: &PreparedFile) -> Result<Self, Error> {
        message.validate().map_err(|_| Error::Denied)?;
        let [item] = message.native_attachments.as_slice() else {
            return Err(Error::Denied);
        };
        let value = Self {
            kind: item.kind,
            sha256: format!("{:x}", Sha256::digest(&file.bytes)),
            bytes: file.bytes.len() as u64,
            filename: file.filename.clone(),
            mime_type: file.mime.clone(),
            caption: file.caption.clone(),
        };
        if !value.valid()
            || value.filename != item.filename.as_deref().unwrap_or("attachment-0")
            || value.mime_type
                != item
                    .mime_type
                    .as_deref()
                    .unwrap_or("application/octet-stream")
            || value.caption != item.caption
        {
            return Err(Error::Denied);
        }
        Ok(value)
    }
    pub(super) fn valid(&self) -> bool {
        // Reuse the public metadata bounds, without retaining the original source.
        let metadata = NativeAttachment {
            kind: self.kind,
            source: NativeMediaSource::Base64 {
                data: "AQ==".into(),
            },
            filename: Some(self.filename.clone()),
            mime_type: Some(self.mime_type.clone()),
            caption: self.caption.clone(),
        };
        self.kind != NativeMediaKind::Sticker
            && self.bytes > 0
            && self.bytes <= MAX_OUTBOUND_BYTES
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && cmaeg_core::message::validate_native_attachments(&[metadata]).is_ok()
    }
}
