//! A compact upload fact on the same durable card; source URLs/bytes are never retained.
use super::Error;
use crate::media::outbound::{PreparedFile, MAX_OUTBOUND_BYTES};
use cmaeg_core::message::{NativeAttachment, NativeMediaKind, NativeMediaSource, OutboundMessage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Media {
    pub(crate) kind: NativeMediaKind,
    sha256: String,
    bytes: u64,
    filename: String,
    mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caption: Option<String>,
    /// Exact native file/presentation proof; absent on historical callback-only cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confirmed: Option<Confirmed>,
    /// New bytes are saved before PATCH, without borrowing the predecessor's proof.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) pending_replacement: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmed {
    attachment_id: String,
    snapshot_sha256: String,
}
impl Media {
    pub(super) fn prepare_item(
        message: &OutboundMessage,
        index: usize,
        file: &PreparedFile,
    ) -> Result<Self, Error> {
        message.validate().map_err(|_| Error::Denied)?;
        if message
            .blocks
            .iter()
            .any(|block| matches!(block, cmaeg_core::message::Block::Form { .. }))
        {
            return Err(Error::Denied);
        }
        let item = message.native_attachments.get(index).ok_or(Error::Denied)?;
        let value = Self {
            kind: item.kind,
            sha256: format!("{:x}", Sha256::digest(&file.bytes)),
            bytes: file.bytes.len() as u64,
            filename: file.filename.clone(),
            mime_type: file.mime.clone(),
            caption: file.caption.clone(),
            confirmed: None,
            pending_replacement: false,
        };
        if !value.valid()
            || value.filename
                != item
                    .filename
                    .as_deref()
                    .unwrap_or(&format!("attachment-{index}"))
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
        !(self.pending_replacement && self.confirmed.is_some())
            && self.confirmed.as_ref().is_none_or(|fact| {
                cmaeg_core::channel::valid_discord_id(&fact.attachment_id)
                    && fact.snapshot_sha256.len() == 64
                    && fact
                        .snapshot_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            && self.kind != NativeMediaKind::Sticker
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

impl Media {
    pub(super) fn valid_for(&self, card: &super::Card) -> bool {
        self.valid()
            && (if self.pending_replacement {
                card.revision > 1
                    && card.reference.is_none()
                    && card.consumed.is_none()
                    && !card.superseded
                    && card.discord_edit.is_none()
            } else {
                self.has_edit_authority() || (card.revision == 1 && card.discord_edit.is_none())
            })
    }

    pub(crate) fn has_edit_authority(&self) -> bool {
        self.confirmed.is_some()
    }
    pub(crate) fn confirm(
        &mut self,
        snapshot: &crate::native_media::model::Snapshot,
    ) -> Result<(), Error> {
        let [file] = snapshot.files.as_slice() else {
            return Err(Error::Denied);
        };
        if snapshot.kind != self.kind
            || file.filename.as_deref() != Some(&self.filename)
            || file
                .mime
                .as_deref()
                .is_some_and(|mime| mime != self.mime_type)
            || file.size != Some(self.bytes)
            || snapshot.caption != self.caption.as_deref().unwrap_or("")
        {
            return Err(Error::Denied);
        }
        self.confirmed = Some(Confirmed {
            attachment_id: file.id.clone(),
            snapshot_sha256: snapshot_hash(snapshot),
        });
        self.pending_replacement = false;
        if !self.valid() {
            return Err(Error::Denied);
        }
        Ok(())
    }
    pub(crate) fn matches_snapshot(&self, snapshot: &crate::native_media::model::Snapshot) -> bool {
        self.confirmed.as_ref().is_some_and(|fact| {
            snapshot.files.len() == 1
                && snapshot.files[0].id == fact.attachment_id
                && snapshot_hash(snapshot) == fact.snapshot_sha256
        })
    }
}
fn snapshot_hash(snapshot: &crate::native_media::model::Snapshot) -> String {
    let mut snapshot = snapshot.clone();
    // Text and control revisions may change; the actual native upload and its
    // observed presentation remain exactly the issued attachment.
    snapshot.content.clear();
    format!(
        "{:x}",
        Sha256::digest(cmaeg_core::profile::canonical_json(
            serde_json::to_value(snapshot).expect("media snapshot serializes")
        ))
    )
}
