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

/// Reset one partition while retaining its transport identity and historical mappings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionResetRequest {
    /// Exact partition key used by inspection.
    pub partition_key: String,
    /// Omitted selects the default consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer_id: Option<String>,
    /// Expected channel record version.
    pub expected_version: u64,
    /// Exact deleted Agent observed by the owner.
    pub expected_agent_id: String,
}
/// Immutable provider reset result, including the lazily provisioned successor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationReset {
    /// Provider binding identity.
    pub binding_id: String,
    /// Unchanged binding generation.
    pub generation: u64,
    /// Unchanged transport conversation identity.
    pub conversation_id: String,
    /// Newly reserved incarnation.
    pub incarnation: u64,
    /// Predecessor Agent identity.
    pub previous_agent_id: String,
    /// Predecessor internal conversation identity.
    pub previous_conversation_id: String,
    /// Reserved successor Agent identity; reset itself does not provision it.
    pub successor_agent_id: String,
    /// Reset receipt state: reserved.
    pub state: String,
}
/// Owner-authorized partition reset response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionResetResponse {
    /// Selected gateway consumer.
    pub consumer_id: String,
    /// Unchanged transport conversation identity.
    pub transport_conversation_id: String,
    /// Immutable provider result.
    pub reset: ConversationReset,
}
