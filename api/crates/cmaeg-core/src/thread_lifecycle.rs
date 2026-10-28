//! Native lifecycle operations never accept caller-selected platform destinations.
use crate::{
    seal::{EventRef, ThreadRef},
    ValidationError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadListView {
    #[default]
    Active,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateThreadRequest {
    pub name: String,
    /// Discord minutes: 60, 1440, 4320, or 10080. Always explicit.
    pub auto_archive_duration: u16,
}
impl CreateThreadRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_name(&self.name)?;
        if !matches!(self.auto_archive_duration, 60 | 1440 | 4320 | 10080) {
            return Err(ValidationError::new(
                "/auto_archive_duration",
                "must be 60, 1440, 4320, or 10080 minutes",
            ));
        }
        Ok(())
    }
}
fn validate_name(name: &str) -> Result<(), ValidationError> {
    if name.trim().is_empty()
        || name.chars().count() > 100
        || name.chars().any(crate::activity::forbidden_text_char)
    {
        return Err(ValidationError::new(
            "/name",
            "must contain 1..100 printable characters",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAction {
    /// Close an admitted Telegram forum topic; never a Discord archive alias.
    Close,
    Archive,
    Reopen,
    Join,
    Leave,
    Rename,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadTarget {
    /// Only the exact thread containing this event, never its siblings.
    Event {
        event_ref: EventRef,
    },
    Thread {
        thread_ref: ThreadRef,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ManageThreadRequest {
    pub target: ThreadTarget,
    pub action: ThreadAction,
    /// Required only for rename: 1–100 printable characters. Omit for every other action.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_name"
    )]
    #[schema(nullable = false, min_length = 1, max_length = 100)]
    pub name: Option<String>,
}
impl ManageThreadRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match (self.action, &self.name) {
            (ThreadAction::Rename, Some(name)) => validate_name(name),
            (ThreadAction::Rename, None) => {
                Err(ValidationError::new("/name", "is required for rename"))
            }
            (_, Some(_)) => Err(ValidationError::new(
                "/name",
                "must be omitted unless action is rename",
            )),
            (_, None) => Ok(()),
        }
    }
}
fn present_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadSummary {
    pub thread_ref: ThreadRef,
    pub thread_id: String,
    pub parent_id: String,
    pub name: String,
    pub archived: bool,
    pub locked: bool,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadList {
    pub threads: Vec<ThreadSummary>,
    /// At most 25 results. There is deliberately no transferable pagination cursor.
    pub truncated: bool,
    pub scope: ThreadListScope,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadListScope {
    OriginalThread,
    PublicChildrenOfOriginalParent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadOutcome {
    Confirmed,
    Unsupported { reason: String },
    Rejected { reason: String },
    RateLimited { retry_after: u64 },
    Unknown { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadReceipt {
    pub outcome: ThreadOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadSummary>,
}

/// Scope availability, never a claim about the bot's current Discord permissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadCapabilities {
    pub create_from_original_message: bool,
    pub list: ThreadListScope,
    pub manage_original_thread: bool,
    pub actions: Vec<ThreadAction>,
    pub expires_at: i64,
}

/// Telegram forum-topic scope only; this grants no listing or creation authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TopicAction {
    Close,
    Reopen,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicLifecycleCapabilities {
    pub manage_original_topic: bool,
    pub actions: Vec<TopicAction>,
    pub expires_at: i64,
}
