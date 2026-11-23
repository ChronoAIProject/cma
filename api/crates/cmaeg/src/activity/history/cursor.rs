use super::{payload::Manifest, repository::Position, Error};
use cmaeg_core::seal::{open_interaction, seal_interaction, MasterKeyRing};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub(super) const HISTORY: &str = "activity-history-page-v1";
pub(super) const DETAILS: &str = "activity-history-details-v1";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PageCursor {
    pub scope: String,
    pub storage_generation: u8,
    pub from_ms: i64,
    pub until_ms: i64,
    pub limit: u16,
    pub watermark: i64,
    pub after: Option<Position>,
    pub deadline_ms: i64,
    pub coverage_revision: i64,
    pub unreadable: bool,
    pub legacy_partial: bool,
    pub retention_days: u16,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DetailCursor {
    pub scope: String,
    pub event_key: String,
    pub manifest: Manifest,
    pub next: usize,
    pub deadline_ms: i64,
    pub expires_ms: i64,
    pub incomplete: bool,
    pub truncated: bool,
}
pub(super) fn seal<T: Serialize>(
    keys: &MasterKeyRing,
    purpose: &str,
    value: &T,
) -> Result<String, Error> {
    let bytes = serde_json::to_vec(value).map_err(|_| Error::InvalidRequest)?;
    let sealed = seal_interaction(keys, purpose, &bytes).map_err(|_| Error::InvalidRequest)?;
    if sealed.len() > 16_384 {
        return Err(Error::InvalidRequest);
    }
    String::from_utf8(sealed).map_err(|_| Error::InvalidRequest)
}
pub(super) fn open<T: DeserializeOwned>(
    keys: &MasterKeyRing,
    purpose: &str,
    token: &str,
) -> Result<T, Error> {
    if token.len() > 16_384 {
        return Err(Error::InvalidRequest);
    }
    let opened =
        open_interaction(keys, purpose, token.as_bytes()).map_err(|_| Error::InvalidRequest)?;
    serde_json::from_slice(&opened.payload).map_err(|_| Error::InvalidRequest)
}
