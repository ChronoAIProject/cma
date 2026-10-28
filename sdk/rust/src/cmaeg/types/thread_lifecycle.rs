use serde::{Deserialize, Serialize};

/// Opaque original-event thread authority. Do not log this reference.
pub type ThreadRef = String;
/// Bounded listing; no destination or pagination cursor may be supplied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadListView {
    /// Active threads (the default).
    #[default]
    Active,
    /// Archived threads.
    Archived,
}
/// Create a public thread anchored to the exact original native human message.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateThreadRequest {
    /// Printable name, 1–100 characters.
    pub name: String,
    /// Explicit Discord minutes: 60, 1440, 4320, or 10080.
    pub auto_archive_duration: u16,
}
/// A supported exact-thread mutation, subject to Discord permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAction {
    /// Close the exact admitted Telegram forum topic. Discord does not support this action.
    Close,
    /// Archive without changing the lock.
    Archive,
    /// Reopen without implicitly unlocking.
    Reopen,
    /// Join as the current bot; requires an active thread.
    Join,
    /// Leave as the current bot; requires an active thread.
    Leave,
    /// Rename an active exact thread without reopening it or changing its lock.
    Rename,
}
/// The only supported destinations for a thread mutation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadTarget {
    /// Exact thread containing the original event.
    Event {
        /// Original unexpired event reference.
        event_ref: super::EventRef,
    },
    /// Exact scoped thread returned by create/list.
    Thread {
        /// Opaque thread authority with the original event expiry.
        thread_ref: ThreadRef,
    },
}
/// Explicit mutation of one scoped thread.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManageThreadRequest {
    /// Original event or sealed thread reference.
    pub target: ThreadTarget,
    /// Action on the thread, or current bot membership.
    pub action: ThreadAction,
    /// Required only for rename: 1–100 printable characters. Omit for every other action.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_name"
    )]
    pub name: Option<String>,
}
fn present_name<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}
/// Bounded current metadata and original-event authority for one thread.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadSummary {
    /// Opaque authority, not transferable to another consumer.
    pub thread_ref: ThreadRef,
    /// Display identity; never an accepted operation destination by itself.
    pub thread_id: String,
    /// Display identity of the exact parent channel.
    pub parent_id: String,
    /// Untrusted platform name.
    pub name: String,
    /// Whether currently archived.
    pub archived: bool,
    /// Whether reopening requires moderator permission.
    pub locked: bool,
    /// Original event expiry in Unix seconds.
    pub expires_at: i64,
}
/// The authority scope retained by a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadListScope {
    /// Only the exact thread containing the original event.
    OriginalThread,
    /// Public threads under the exact original ordinary parent channel.
    PublicChildrenOfOriginalParent,
}
/// At most 25 thread results; no private sibling enumeration or pagination.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadList {
    /// Bounded metadata and authority.
    pub threads: Vec<ThreadSummary>,
    /// More results exist beyond this fixed window.
    pub truncated: bool,
    /// Scope inherited from the original event.
    pub scope: ThreadListScope,
}
/// Persisted outcome. Unknown writes must never be retried under a new key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThreadOutcome {
    /// The platform confirmed the operation.
    Confirmed,
    /// The requested operation is outside the supported scope.
    Unsupported {
        /// Stable machine-readable refusal reason.
        reason: String,
    },
    /// Definite refusal, including missing platform access or permissions.
    Rejected {
        /// Stable machine-readable refusal reason.
        reason: String,
    },
    /// The platform refused due to its rate limit.
    RateLimited {
        /// Suggested delay in seconds.
        retry_after: u64,
    },
    /// The write may have occurred; its destination remains fenced.
    Unknown {
        /// Stable machine-readable uncertainty reason.
        reason: String,
    },
}
/// Durable thread operation result.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadReceipt {
    /// Confirmed, unsupported, rejected, rate limited, or unknown.
    pub outcome: ThreadOutcome,
    /// Present only when current metadata and retained authority were confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadSummary>,
}
/// Available scope, independent of actual bot permissions on Discord.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadCapabilities {
    /// Public message-anchored creation is supported for this original event.
    pub create_from_original_message: bool,
    /// Scope of list results.
    pub list: ThreadListScope,
    /// The event itself targets a manageable thread.
    pub manage_original_thread: bool,
    /// Supported actions on exact scoped thread references.
    pub actions: Vec<ThreadAction>,
    /// Original event expiry in Unix seconds.
    pub expires_at: i64,
}

/// Exact admitted Telegram forum topic, without creation or enumeration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopicLifecycleCapabilities {
    /// The original event targets a manageable forum topic.
    pub manage_original_topic: bool,
    /// Telegram close/reopen only; distinct from Discord archiving.
    pub actions: Vec<TopicAction>,
    /// Original event expiry in Unix seconds.
    pub expires_at: i64,
}
/// Supported Telegram topic operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopicAction {
    /// Close the original topic.
    Close,
    /// Reopen the original topic.
    Reopen,
}
