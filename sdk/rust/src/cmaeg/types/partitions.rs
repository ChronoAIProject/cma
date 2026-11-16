use serde::{Deserialize, Serialize};

/// Canonical provider mapping and current Agent lifecycle.
pub type ConversationInspection = crate::provider_bindings::BindingConversationInspection;

/// A read-only channel partition mapping from the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionInspection {
    /// Selected gateway consumer.
    pub consumer_id: String,
    /// Gateway transport conversation before provider affinity resolution.
    pub transport_conversation_id: String,
    /// Retained provider mapping and mapped Agent lifecycle state.
    pub mapping: ConversationInspection,
}

/// Exact owner-scoped head; inspection grants no release authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadPreview {
    /// Opaque durable partition identity.
    pub partition_id: String,
    /// Current unresolved work identity.
    pub head: String,
    /// Current partition fencing epoch.
    pub epoch: u64,
    /// Current partition record version.
    pub version: u64,
    /// Observed invocation or delayed-effect phase.
    pub phase: String,
    /// Original event or delayed capability expiry, in Unix seconds.
    pub event_expires_at: i64,
    /// Whether release is enabled and this snapshot is eligible.
    pub releasable: bool,
}

/// Release requires an explicit recovery grant and the current head proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRequest {
    /// Exact partition from the preview.
    pub partition_id: String,
    /// Exact work identity from the preview.
    pub expected_head: String,
    /// Fencing epoch from the preview.
    pub expected_epoch: u64,
    /// Record version from the preview.
    pub expected_version: u64,
    /// Stable UUID for this one release operation and its retries.
    pub operation_id: String,
    /// Caller attestation of terminal provider evidence.
    pub provider_terminal_confirmed: bool,
    /// Explicit consent to quarantine stale delayed replies.
    pub suppress_delayed_replies: bool,
}

/// An audited disposition preserves uncertainty; it is not delivery evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseResult {
    /// UUID of the committed release operation.
    pub operation_id: String,
    /// The single work identity removed from the head.
    pub released_head: String,
    /// Audited unresolved disposition; never a confirmed delivery result.
    pub disposition: String,
    /// Fresh fencing epoch used for this release.
    pub epoch: u64,
    /// Gateway storage time of release, in Unix milliseconds.
    pub released_at_ms: i64,
}
