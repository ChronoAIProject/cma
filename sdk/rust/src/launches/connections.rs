//! Owner-only service connection metadata and hosted handoffs.

use serde::{Deserialize, Serialize};

string_enum! { /// Current server-observed connection requirement.
    pub enum RequirementState { Satisfied => "satisfied", Inactive => "inactive", Missing => "missing", Unknown => "unknown" }
}
string_enum! { /// Provider-supported connection setup mechanism.
    pub enum ConnectionMode { Hosted => "hosted", Manual => "manual", Unknown => "unknown" }
}
string_enum! { /// Hosted connection handoff lifecycle.
    pub enum ConnectionState { Pending => "pending", Satisfied => "satisfied", Expired => "expired", Cancelled => "cancelled", Failed => "failed" }
}

/// Durable connection requirement without provider-specific metadata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServiceRequirement {
    /// Required service identity.
    pub service: String,
    /// Observed connection state.
    pub state: RequirementState,
}

/// Durable service requirements at their last observation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Requirements {
    /// Required services.
    pub services: Vec<ServiceRequirement>,
    /// Observation timestamp.
    pub checked_at: String,
}

/// Ephemeral owner-only display and setup metadata, never persisted in Launch state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServiceConnectionMetadata {
    /// Required service identity.
    pub service: String,
    /// Observed connection state.
    pub state: RequirementState,
    /// Human-readable service name.
    pub display_name: Option<String>,
    /// Available connection flow.
    pub connection_mode: Option<ConnectionMode>,
    /// Owner-only setup link when the provider supplies one.
    pub setup_url: Option<String>,
}

/// Fresh owner-only connection setup requirements.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectionRequirements {
    /// Service states with ephemeral connection metadata.
    pub services: Vec<ServiceConnectionMetadata>,
    /// Observation timestamp.
    pub checked_at: String,
}

/// Explicit request for one hosted connection handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateConnectionRequest {
    /// Service to connect.
    pub service_id: String,
    /// Owner's Launch requiring the service.
    pub launch_id: String,
    /// Stable caller intent for this exact service and Launch.
    pub intent_id: String,
}

/// Hosted handoff created without following or fetching its external URL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedConnection {
    /// Handoff identity.
    pub handoff_id: String,
    /// Hosted setup URL for explicit user navigation.
    pub hosted_url: String,
    /// Handoff expiration timestamp.
    pub expires_at: String,
}

/// Current owner-only connection handoff status.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConnectionStatus {
    /// Handoff identity.
    pub handoff_id: String,
    /// Associated Launch identity.
    pub launch_id: String,
    /// Associated service identity.
    pub service_id: String,
    /// Current lifecycle.
    pub state: ConnectionState,
    /// Expiration timestamp.
    pub expires_at: String,
    /// Last observation timestamp.
    pub updated_at: String,
}
