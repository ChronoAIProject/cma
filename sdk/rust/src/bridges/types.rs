//! Public management projections and explicit mutation requests.
use super::{BridgeAcceptedWork, BridgeConfiguration, BridgePlatform};
use crate::resource_access::{ResourceActor, ResourceOwner};
use serde::{Deserialize, Serialize};

/// Independent reversible bridge controls.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeEmergencyControls {
    /// Stop accepting new callbacks.
    pub ingress_stopped: bool,
    /// Stop Agent execution through this bridge.
    pub execution_stopped: bool,
    /// Stop outbound delivery through this bridge.
    pub outbound_stopped: bool,
}
/// Current definition lifecycle, independent of resource lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeState {
    /// New ingress is enabled, subject to current authority and emergency controls.
    Enabled,
    /// New ingress is disabled.
    Disabled {
        /// Explicit handling of accepted work.
        accepted_work: BridgeAcceptedWork,
    },
    /// The definition is permanently retired.
    Retired {
        /// Explicit handling of accepted work.
        accepted_work: BridgeAcceptedWork,
    },
}
/// Safe bridge projection for a current member or owner.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Bridge {
    /// Exact bridge identity.
    pub id: String,
    /// Retained person or organization owner.
    pub owner: ResourceOwner,
    /// Human-readable name.
    pub display_name: String,
    /// Positive current definition revision.
    pub revision: u64,
    /// Immutable selected configuration generation.
    pub config_revision: u64,
    /// Current definition state.
    pub state: BridgeState,
    /// Independent current stop controls.
    pub emergency: BridgeEmergencyControls,
    /// Selected existing bot identity.
    pub bot_id: String,
    /// Selected existing bot platform.
    pub platform: BridgePlatform,
    /// Current management capability, rechecked by later requests.
    pub can_manage: bool,
    /// Original creation timestamp.
    pub created_at: String,
    /// Latest definition update timestamp.
    pub updated_at: String,
}
/// Ordered bounded inventory with an exclusive bridge-identity cursor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgePage {
    /// Current visible bridges.
    pub items: Vec<Bridge>,
    /// Last returned identity when another page exists.
    pub next_cursor: Option<String>,
}
/// Safe key-custody state, never a key or custody reference.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeCredentialStatus {
    /// Local custody revision; zero means absent.
    pub revision: u64,
    /// Whether a custody entry exists, including retained revocation.
    pub present: bool,
    /// Whether the retained entry was explicitly revoked.
    pub revoked: bool,
}
/// Management-only configuration and server-derived callback URL.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeConfigurationDetail {
    /// Current manageable bridge projection.
    pub bridge: Bridge,
    /// Selected configuration, including historically retained policy syntax.
    pub config: BridgeConfiguration,
    /// Server-derived HTTPS callback destination for inspection.
    pub callback_url: String,
    /// Safe current selected-key custody state.
    pub credential: BridgeCredentialStatus,
}
/// Durable receipt for one explicit definition mutation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeReceipt {
    /// Exact mutated bridge identity.
    pub bridge_id: String,
    /// Retained owner.
    pub owner: ResourceOwner,
    /// Actual human/API actor; callback senders do not become owners.
    pub actor: ResourceActor,
    /// Committed definition revision.
    pub revision: u64,
    /// Committed configuration generation.
    pub config_revision: u64,
    /// Exact applied management command.
    pub command: String,
    /// Committed mutation timestamp.
    pub applied_at: String,
}
/// Fresh validation result, not deferred execution authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeValidation {
    /// Exact inspected bridge.
    pub bridge_id: String,
    /// Exact inspected definition revision.
    pub revision: u64,
    /// Exact inspected configuration generation.
    pub config_revision: u64,
    /// Successful live validation at request time.
    pub valid: bool,
}
/// Create one disabled draft from explicitly selected existing provider objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBridgeRequest {
    /// Actual person or organization owner scope.
    pub owner: ResourceOwner,
    /// Explicit fresh configuration.
    pub config: BridgeConfiguration,
}
/// Replace configuration under an exact definition revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateBridgeRequest {
    /// Exact observed definition revision.
    pub expected_revision: u64,
    /// Explicit fresh configuration.
    pub config: BridgeConfiguration,
    /// Explicit old-work behavior where rebinding requires it.
    pub previous_work: Option<BridgeAcceptedWork>,
}
/// Explicit management control; it grants no resource provisioning authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeControl {
    /// Enable ingress after current connection checks.
    Enable,
    /// Disable ingress with explicit accepted-work behavior.
    Disable {
        /// Treatment of accepted work.
        accepted_work: BridgeAcceptedWork,
    },
    /// Retire the definition, retaining resources and history.
    Retire {
        /// Treatment of accepted work.
        accepted_work: BridgeAcceptedWork,
    },
    /// Set all independent emergency controls explicitly.
    Emergency {
        /// Current desired stop controls.
        controls: BridgeEmergencyControls,
    },
}
impl BridgeControl {
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable { .. } => "disable",
            Self::Retire { .. } => "retire",
            Self::Emergency { .. } => "emergency",
        }
    }
}
/// Apply one explicit control under current management authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlBridgeRequest {
    /// Exact observed definition revision.
    pub expected_revision: u64,
    /// Explicit desired control.
    pub command: BridgeControl,
}
/// Borrowed write-only selected key. The SDK client never retains this request.
#[derive(Serialize)]
pub struct ImportBridgeKeyRequest<'a> {
    /// Exact observed definition revision.
    pub expected_revision: u64,
    /// Exact selected key, one through 4096 bytes; never returned or included in Debug.
    pub key: &'a str,
}
impl std::fmt::Debug for ImportBridgeKeyRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportBridgeKeyRequest")
            .field("expected_revision", &self.expected_revision)
            .field("key", &"[redacted]")
            .finish()
    }
}
