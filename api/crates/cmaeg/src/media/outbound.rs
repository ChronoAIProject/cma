//! Bounded attachment materialization before the delivery commit point.

use crate::media::{
    body::{read_capped, Overflow},
    source::url::{PinnedTransport, UrlTransport},
    FetchError,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::message::{NativeAttachment, NativeMediaSource};
use std::time::Duration;
use tokio::time::Instant;

/// Bytes are prepared before delivery commits; credentials never enter file parts.
pub struct PreparedFile {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub mime: String,
    pub caption: Option<String>,
}

pub const MAX_OUTBOUND_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) async fn prepare(items: &[NativeAttachment]) -> Result<Vec<PreparedFile>, FetchError> {
    prepare_with(items, &PinnedTransport).await
}
pub(crate) async fn prepare_with(
    items: &[NativeAttachment],
    transport: &dyn UrlTransport,
) -> Result<Vec<PreparedFile>, FetchError> {
    cmaeg_core::message::validate_native_attachments(items).map_err(|_| FetchError::Rejected)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut remaining = MAX_OUTBOUND_BYTES;
    let mut files = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let bytes = match &item.source {
            NativeMediaSource::Base64 { data } => {
                STANDARD.decode(data).map_err(|_| FetchError::Rejected)?
            }
            NativeMediaSource::Url { url } => {
                let url = reqwest::Url::parse(url).map_err(|_| FetchError::Rejected)?;
                let response = transport.get(&url, deadline).await?;
                if !response.status().is_success() {
                    return Err(FetchError::Rejected);
                }
                read_capped(response, remaining, Overflow::Reject, deadline)
                    .await?
                    .bytes
                    .to_vec()
            }
        };
        if bytes.is_empty() || bytes.len() as u64 > remaining {
            return Err(FetchError::TooLarge);
        }
        remaining -= bytes.len() as u64;
        files.push(PreparedFile {
            bytes,
            filename: item
                .filename
                .clone()
                .unwrap_or_else(|| format!("attachment-{index}")),
            mime: item
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".into()),
            caption: item.caption.clone(),
        });
    }
    Ok(files)
}
