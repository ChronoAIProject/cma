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
