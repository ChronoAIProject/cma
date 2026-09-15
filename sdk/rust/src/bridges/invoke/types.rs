//! Safe explicit consent projections. None of these records is execution authority.
use super::super::BridgeAgentTarget;
use crate::resource_access::{ResourceActor, ResourceOwner};
use serde::{Deserialize, Serialize};

string_enum! { /// Required explicit consent for a retained Agent's existing conversation context.
    pub enum BridgeSharedContextConsent {
        UseRetainedAgentContext => "use_retained_agent_context",
        UseChannelCreatedAgent => "use_channel_created_agent"
    }
}
string_enum! { /// Current presence of usable explicit consent, independent of live execution permission.
    pub enum BridgeInvokeConsentStatus { NotApplicable => "not_applicable", ConsentRequired => "consent_required", ConsentPresent => "consent_present" }
}
/// Immutable configuration generation to which consent was issued.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokePin {
    /// Exact owning bridge.
    pub bridge_id: String,
    /// Original selected configuration generation, one through 256.
    pub config_revision: u64,
}
/// Exact retained creation lineage selected when consent was issued.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeTarget {
    /// Exact Sandbox/Workspace/Worktree/Agent IDs.
    pub ids: BridgeAgentTarget,
    /// Original Sandbox creation timestamp.
    pub sandbox_created_at: String,
    /// Original Workspace creation timestamp.
    pub workspace_created_at: String,
    /// Original Agent creation timestamp.
    pub agent_created_at: String,
    /// Lowercase SHA-256 creation provenance fingerprint.
    pub agent_creation_fingerprint: String,
    /// Lowercase SHA-256 identity digest of the retained Worktree.
    pub worktree_identity_digest: String,
}
/// Grant lifecycle. Revocation is terminal and does not rewrite original consent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "GrantStateWire")]
pub enum BridgeInvokeGrantState {
    /// Original grant, at revision one.
    Active,
    /// Explicit terminal revocation, at revision two.
    Revoked {
        /// Original revocation timestamp.
        at: String,
        /// Actual human/API manager who revoked the grant.
        actor: ResourceActor,
    },
}

// A unit variant in Serde's internally tagged representation ignores extra
// fields. A struct variant closes that wire shape without changing the public API.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum GrantStateWire {
    Active {},
    Revoked { at: String, actor: ResourceActor },
}
impl From<GrantStateWire> for BridgeInvokeGrantState {
    fn from(value: GrantStateWire) -> Self {
        match value {
            GrantStateWire::Active {} => Self::Active,
            GrantStateWire::Revoked { at, actor } => Self::Revoked { at, actor },
        }
    }
}
/// Management-only consent history without connector, custody or credential material.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeGrant {
    /// Stable `brinv_` followed by a lowercase SHA-256 digest.
    pub id: String,
    /// One when active, two when revoked.
    pub revision: u64,
    /// Retained bridge-compatible owner.
    pub owner: ResourceOwner,
    /// Exact original configuration pin.
    pub pin: BridgeInvokePin,
    /// Exact target lineage.
    pub target: BridgeInvokeTarget,
    /// Explicit original shared-context consent.
    pub consent: BridgeSharedContextConsent,
    /// Original issuance timestamp.
    pub issued_at: String,
    /// Actual human/API manager who issued consent.
    pub issued_by: ResourceActor,
    /// Current active/revoked status.
    pub state: BridgeInvokeGrantState,
}
/// Idempotent issue/revoke receipt with exact attributed result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeReceipt {
    /// Lowercase SHA-256 durable receipt identity.
    pub id: String,
    /// Lowercase SHA-256 digest of the exact original request.
    pub request_digest: String,
    /// Exact bridge affected by this grant command.
    pub bridge_id: String,
    /// Actual human/API actor for this command.
    pub actor: ResourceActor,
    /// Safe committed grant projection.
    pub result: BridgeInvokeGrant,
}
/// Ordered management-only grant history, including revoked entries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokePage {
    /// Grants in strictly increasing ID order.
    pub items: Vec<BridgeInvokeGrant>,
    /// Exact last returned grant ID when another page exists.
    pub next_cursor: Option<String>,
}
/// Current consent observation. Connection, enablement and each execution effect
/// still require their own current server checks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeInvokeReadiness {
    /// Exact inspected bridge.
    pub bridge_id: String,
    /// Inspected current definition revision.
    pub revision: u64,
    /// Inspected current configuration generation.
    pub config_revision: u64,
    /// Whether the selected placement needs or currently has consent.
    pub status: BridgeInvokeConsentStatus,
    /// Active current-generation consent only when status is ConsentPresent.
    pub candidate: Option<BridgeInvokeGrant>,
}
/// Explicit actual-manager request to use a retained Agent's shared context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueBridgeInvokeGrantRequest {
    /// Exact observed bridge definition revision.
    pub expected_bridge_revision: u64,
    /// Exact retained target selected by the current bridge configuration.
    pub target: BridgeAgentTarget,
    /// Required explicit consent; there is no default or inferred acknowledgement.
    pub shared_context: BridgeSharedContextConsent,
}
/// Explicit terminal revocation under exact observed bridge and grant revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeBridgeInvokeGrantRequest {
    /// Exact observed bridge definition revision.
    pub expected_bridge_revision: u64,
    /// Exact observed grant revision.
    pub expected_revision: u64,
}
