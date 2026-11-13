//! Provider selection and binding wire types.
use super::{BindingId, ConformanceLevel, ProviderSlug};
use serde::{Deserialize, Serialize};

/// Provider-owned selection of a provisioned profile or a borrowed existing Agent.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderTarget {
    /// The provider provisions resources from the EventWorkerProfile.
    Profile,
    /// Every sender is routed to this existing Agent without modifying its resources.
    ExistingAgent {
        /// Existing CMA Agent identifier, owned by the channel owner.
        agent_id: String,
    },
}

/// CMAEG `ProviderBinding` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderBinding {
    /// Omitted selects the legacy profile provisioning behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProviderTarget>,
    /// Wire field `binding_id`.
    pub binding_id: BindingId,
    /// Wire field `level`.
    pub level: ConformanceLevel,
    /// Wire field `slug`.
    pub slug: ProviderSlug,
}

/// CMAEG `ProviderRequest` wire shape.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    /// Omitted selects the legacy profile provisioning behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProviderTarget>,
    /// Wire field `slug`.
    pub slug: ProviderSlug,
}
