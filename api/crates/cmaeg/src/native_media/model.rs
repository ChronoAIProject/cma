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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub kind: NativeMediaKind,
    /// Telegram PhotoSize variants describe one photo, not multiple attachments.
    pub files: Vec<FileFact>,
    pub caption: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<Presentation>,
}
impl Snapshot {
    /// Historical records never acquire newly observed decoration authority.
    pub fn for_revision(mut self, previous: &Self) -> Option<Self> {
        if previous.presentation.is_none() {
            if !self.presentation.as_ref().is_none_or(Presentation::plain) {
                return None;
            }
            self.presentation = None;
            for file in &mut self.files {
                file.metadata = None;
            }
        }
        Some(self)
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "platform", rename_all = "snake_case")]
pub(crate) enum Presentation {
    Telegram {
        entities: Vec<CaptionEntity>,
        caption_above: bool,
        spoiler: bool,
    },
    Discord {
        message_type: u64,
        reply: Option<serde_json::Value>,
        embeds: Vec<serde_json::Value>,
        flags: u64,
        attachment_flags: u64,
    },
}
impl Presentation {
    fn plain(&self) -> bool {
        match self {
            Self::Telegram {
                entities,
                caption_above,
                spoiler,
            } => entities.is_empty() && !caption_above && !spoiler,
            Self::Discord {
                embeds,
                flags,
                attachment_flags,
                ..
            } => embeds.is_empty() && *flags == 0 && *attachment_flags == 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptionEntity {
    pub r#type: String,
    pub offset: u32,
    pub length: u32,
}
pub(crate) struct Observation {
    pub snapshot: Snapshot,
    pub eligible: bool,
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
    /// Absence is the legacy ready revision; an admitted write closes it durably.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RevisionStatus>,
    pub expires_at: i64,
    #[serde(skip)]
    pub version: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "state", rename_all = "snake_case")]
pub(crate) enum RevisionStatus {
    Attempting {
        operation: String,
        revision: u64,
        snapshot_sha256: String,
    },
    Retired {
        operation: String,
        reason: String,
    },
}
impl Current {
    pub fn ready(&self) -> bool {
        self.status.is_none()
    }
    pub fn owns_attempt(&self, operation: &str) -> bool {
        matches!(&self.status, Some(RevisionStatus::Attempting {
            operation: active, revision, snapshot_sha256
        }) if active == operation && *revision == self.revision
            && *snapshot_sha256 == crate::tools::message_effects::mutation::hash(&self.snapshot))
    }
}
