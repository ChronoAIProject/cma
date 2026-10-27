//! Bounded normalized provider facts; no source bytes or credentials are persisted.
use cmaeg_core::{activity::Activity, message::NativeMediaKind, seal::EventRefPayload};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileFact {
    pub id: String,
    pub locator: Option<String>,
    pub filename: Option<String>,
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub kind: NativeMediaKind,
    /// Telegram PhotoSize variants describe one photo, not multiple attachments.
    pub files: Vec<FileFact>,
    pub caption: String,
    pub content: String,
}
impl Snapshot {
    pub fn same_files(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.files.len() == other.files.len()
            && self.files.iter().zip(&other.files).all(|(a, b)| {
                let mut a = a.clone();
                let mut b = b.clone();
                a.locator = None;
                b.locator = None;
                a == b
            })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Original {
    pub event: EventRefPayload,
    pub activity: Activity,
    pub owner: String,
    pub discord_channel: Option<serde_json::Value>,
    pub authority_sha256: String,
    pub message_id: String,
    pub snapshot: Snapshot,
    pub confirmed_at: i64,
    pub expires_at: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Current {
    pub original_sha256: String,
    pub revision: u64,
    pub snapshot: Snapshot,
    pub last_operation: Option<String>,
    pub expires_at: i64,
    #[serde(skip)]
    pub version: u64,
}
