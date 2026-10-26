//! Explicit opt-in native-thread policy for CMAEG channels.
use super::ActorKind;
use serde::{Deserialize, Serialize};

/// Opt-in native-thread metadata and bounded per-chat admission.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadContract {
    /// Negotiated contract version (currently 1).
    pub version: u16,
    #[serde(default)]
    /// At most 256 unique chat IDs admitted for consumer-owned follow policy.
    pub follow_chat_ids: Vec<String>,
}

/// Verified native-thread metadata, available only after explicit negotiation.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadFacts {
    /// Contract version (currently 1).
    pub version: u16,
    /// Verified native thread surface.
    pub kind: ThreadKind,
    /// Original platform chat identifier.
    pub chat_id: String,
    /// Original platform message identifier.
    pub message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Canonical root, absent if ancestry is unresolved.
    pub root_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Optional native alias; never a credential.
    pub native_thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Immediate parent, which is not assumed to be the root.
    pub parent_message_id: Option<String>,
    /// Verified sender classification.
    pub sender_kind: ActorKind,
    /// Evidence of addressing this bot.
    pub address: ThreadAddress,
    #[serde(default)]
    /// Only other users were mentioned, without the bot or everyone.
    pub mentions_others: bool,
}

/// Thread surfaces implemented by this contract.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKind {
    /// A native platform thread.
    Native,
}
/// Verified addressing evidence; admission alone is never such evidence.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadAddress {
    /// An exact structured bot mention.
    Mention,
    /// A verified reply to the bot.
    ReplyToBot,
    /// No bot-addressing evidence.
    NotAddressed,
    /// Addressing evidence unavailable.
    Unknown,
}
