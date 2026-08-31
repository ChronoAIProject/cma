//! Durable references to one coherent chat-host recovery point.

use serde::{Deserialize, Serialize};

use crate::SessionId;

/// One exact conversation object belonging to a host checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCheckpointConversationRef {
    pub session_id: SessionId,
    pub object_reference: String,
    pub sha256: String,
    pub conversation_id: String,
}

/// Immutable conversation and workspace objects sealed as one recovery point.
///
/// Validation and capture land in #752. Defining the complete reference now
/// keeps the host record stable while that behavior is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCheckpointRef {
    pub workspace_object_reference: String,
    pub workspace_archive_sha256: String,
    pub workspace_manifest_digest: String,
    pub conversations: Vec<ChatCheckpointConversationRef>,
    pub captured_at: String,
    pub checkpoint_version: u64,
    pub pair_sha256: String,
}
