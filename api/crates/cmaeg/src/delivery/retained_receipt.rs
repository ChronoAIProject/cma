//! Exact split-text references retained in bounded sealed records. Pages precede
//! their receipt pointer; an incomplete write never proves delivery or permits resend.
use super::intents::{ConfirmedMessagePart, IntentReceipt};
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    message::{MAX_BLOCKS, MAX_MESSAGE_TEXT_BYTES},
    profile::canonical_json,
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ops::Deref;

pub(crate) const INLINE_LIMIT: usize = 16 * 1024;
// Escaping doubles at most 17 text sections. Two consecutive renderer chunks
// consume at least 1998 UTF-16 units (Discord's smaller 2000-unit limit).
// The 128 KiB public request and 256 KiB retained automatic effect limits also
// bound link URLs, whose individual core field has no smaller length limit.
const MAX_PARTS: usize =
    2 * (2 * MAX_MESSAGE_TEXT_BYTES * (MAX_BLOCKS + 1) + 2 * MAX_BLOCKS).div_ceil(1998) + 1;
// Even eight maximum-size MessageRefs fit well below the 49 KiB sealed-record limit.
const PAGE_PARTS: usize = 8;
const KIND: &str = "native_split_text_receipt_page_v1";

pub use cmaeg_engine::{PartsManifest, StoredReceipt};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    kind: String,
    id: String,
    scope: String,
    owner: String,
    retained_until: i64,
    receipt_sha256: String,
    page: usize,
    parts: Vec<ConfirmedMessagePart>,
}

pub(crate) struct ReceiptStore<'a> {
    pub records: &'a RecordClient,
    pub keys: &'a MasterKeyRing,
    pub scope: &'a str,
    pub owner: &'a str,
    pub retained_until: i64,
}
impl ReceiptStore<'_> {
    pub async fn retain(&self, receipt: &IntentReceipt) -> Result<StoredReceipt, RecordError> {
        let bytes = encoded(receipt)?;
        validate_parts(&receipt.message_parts)?;
        if receipt.message_parts.is_empty() || bytes.len() <= INLINE_LIMIT {
            return Ok(StoredReceipt::Inline(receipt.clone()));
        }
        let mut base = receipt.clone();
        let parts = std::mem::take(&mut base.message_parts);
        if parts.is_empty() || encoded(&base)?.len() > INLINE_LIMIT - 1024 {
            return Err(invalid());
        }
        let manifest = PartsManifest {
            kind: KIND.into(),
            scope: hash(self.scope.as_bytes()),
            owner: self.owner.into(),
            retained_until: self.retained_until,
            receipt_sha256: hash(&bytes),
            count: parts.len(),
        };
        for (index, parts) in parts.chunks(PAGE_PARTS).enumerate() {
            let page = Page {
                kind: KIND.into(),
                id: page_id(&manifest, index),
                scope: manifest.scope.clone(),
                owner: manifest.owner.clone(),
                retained_until: manifest.retained_until,
                receipt_sha256: manifest.receipt_sha256.clone(),
                page: index,
                parts: parts.to_vec(),
            };
            let plaintext = encoded(&page)?;
            let blob = seal_interaction(self.keys, &page.id, &plaintext).map_err(|_| invalid())?;
            match self
                .records
                .put_expiring(
                    &RecordId::new(&page.id).map_err(|_| invalid())?,
                    self.owner,
                    &blob,
                    None,
                    Some(self.retained_until),
                )
                .await
            {
                Ok(1) => {}
                Ok(_) => return Err(invalid()),
                Err(_) => {
                    // A lost create reply is confirmed only by the exact sealed page.
                    let (saved, version) = self.read_page(&page.id).await?;
                    if version != 1 || encoded(&saved)? != plaintext {
                        return Err(invalid());
                    }
                }
            }
        }
        Ok(StoredReceipt::Paged {
            receipt: base,
            message_parts_pages: Box::new(manifest),
        })
    }

    pub async fn load(&self, stored: &StoredReceipt) -> Result<IntentReceipt, RecordError> {
        let StoredReceipt::Paged {
            receipt,
            message_parts_pages: manifest,
        } = stored
        else {
            return Ok(stored.deref().clone());
        };
        if manifest.kind != KIND
            || manifest.scope != hash(self.scope.as_bytes())
            || manifest.owner != self.owner
            || manifest.retained_until != self.retained_until
            || manifest.count == 0
            || manifest.count > MAX_PARTS
            || manifest.receipt_sha256.len() != 64
            || !manifest
                .receipt_sha256
                .bytes()
                .all(|v| v.is_ascii_hexdigit())
            || !receipt.message_parts.is_empty()
        {
            return Err(invalid());
        }
        let mut receipt = receipt.clone();
        for index in 0..manifest.count.div_ceil(PAGE_PARTS) {
            let id = page_id(manifest, index);
            let (page, version) = self.read_page(&id).await?;
            if version != 1
                || page.kind != KIND
                || page.id != id
                || page.scope != manifest.scope
                || page.owner != manifest.owner
                || page.retained_until != manifest.retained_until
                || page.receipt_sha256 != manifest.receipt_sha256
                || page.page != index
                || page.parts.len() != PAGE_PARTS.min(manifest.count - index * PAGE_PARTS)
            {
                return Err(invalid());
            }
            receipt.message_parts.extend(page.parts);
        }
        validate_parts(&receipt.message_parts)?;
        if hash(&encoded(&receipt)?) != manifest.receipt_sha256 {
            return Err(invalid());
        }
        Ok(receipt)
    }

    async fn read_page(&self, id: &str) -> Result<(Page, u64), RecordError> {
        let value = self
            .records
            .get(self.records.record_url(id)?.as_str())
            .await?;
        let opened = open_interaction(self.keys, id, &value.blob).map_err(|_| invalid())?;
        let page = serde_json::from_slice(&opened.payload).map_err(|_| invalid())?;
        Ok((page, value.version))
    }
}
fn validate_parts(parts: &[ConfirmedMessagePart]) -> Result<(), RecordError> {
    if parts.len() > MAX_PARTS
        || parts.iter().enumerate().any(|(index, part)| {
            usize::from(part.index) >= MAX_PARTS
                || (index > 0 && parts[index - 1].index >= part.index)
                || !part.message_ref.as_str().starts_with("cmaeg1.mr.")
        })
    {
        return Err(invalid());
    }
    Ok(())
}
fn page_id(manifest: &PartsManifest, page: usize) -> String {
    let digest = Sha256::digest(
        encoded(&(
            KIND,
            &manifest.scope,
            &manifest.owner,
            manifest.retained_until,
            &manifest.receipt_sha256,
            page,
        ))
        .expect("fixed receipt identity serializes"),
    );
    let mut id: [u8; 16] = digest[..16].try_into().unwrap();
    id[6] = (id[6] & 0x0f) | 0x40;
    id[8] = (id[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(id).to_string()
}
fn encoded(value: &impl Serialize) -> Result<Vec<u8>, RecordError> {
    Ok(canonical_json(serde_json::to_value(value).map_err(|_| invalid())?).into_bytes())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn invalid() -> RecordError {
    RecordError::Malformed("split text receipt pages are unavailable or inconsistent".into())
}

#[cfg(test)]
#[path = "retained_receipt_tests.rs"]
mod tests;
