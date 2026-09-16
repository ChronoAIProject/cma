use crate::resource_access::ResourceActor;
use serde::{Deserialize, Serialize};
string_enum! { /// Durable outbound status; only Sent confirms platform delivery.
    pub enum BridgeDeliveryStatus { Pending => "pending", Claimed => "claimed", Sending => "sending", RetryWait => "retry_wait", Sent => "sent", Rejected => "rejected", Unknown => "unknown", Abandoned => "abandoned" }
}
string_enum! { /// Safe delivery failure codes, excluding provider bodies and message text.
    pub enum BridgeDeliveryFailure { CredentialExpired => "credential_expired", CredentialRevoked => "credential_revoked", BotInactive => "bot_inactive", RouteUnavailable => "route_unavailable", CapabilityUnsupported => "capability_unsupported", RateLimited => "rate_limited", RequestRejected => "request_rejected", ConnectUnavailable => "connect_unavailable", ProviderFailure => "provider_failure", ResponseLost => "response_lost", WorkerLost => "worker_lost", ReceiptInvalid => "receipt_invalid" }
}
/// One retained delivery attempt without secret references or routing context.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryAttempt {
    /// Stable local attempt identity.
    pub id: String,
    /// One-based attempt order, at most sixteen.
    pub number: u32,
    /// Original attempt time in Unix milliseconds.
    pub started_at_ms: u64,
    /// Last committed state of this attempt.
    pub status: BridgeDeliveryStatus,
    /// Bounded failure classification, if any.
    pub failure: Option<BridgeDeliveryFailure>,
    /// Whether this attempt used deliberately retained matching key custody.
    pub retained_key: bool,
}
/// Safe owner/admin delivery history. This record confers no sending authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDelivery {
    /// Stable local outbound item identity.
    pub id: String,
    /// Original Bridge identity.
    pub bridge_id: String,
    /// Original configuration generation.
    pub config_revision: u64,
    /// Permanent explicit Agent reply invocation receipt.
    pub receipt_id: String,
    /// Original common command response identity.
    pub response_id: String,
    /// Current item revision for compare-and-swap recovery.
    pub revision: u64,
    /// Original order within the response.
    pub sequence: u64,
    /// Zero-based chunk order within this explicit reply invocation.
    pub chunk_index: u32,
    /// Total chunks in this invocation, at most 1024.
    pub chunk_count: u32,
    /// Current durable status. Unknown is never automatically resent.
    pub status: BridgeDeliveryStatus,
    /// Bounded failure classification, if any.
    pub failure: Option<BridgeDeliveryFailure>,
    /// A manager explicitly retried an uncertain delivery.
    pub duplicate_risk: bool,
    /// Scheduled safe retry in Unix milliseconds, only for RetryWait.
    pub next_retry_at_ms: Option<u64>,
    /// Ordered retained attempts, including earlier Unknown results.
    pub attempts: Vec<BridgeDeliveryAttempt>,
    /// Original item creation time in Unix milliseconds.
    pub created_at_ms: u64,
    /// Last state transition time in Unix milliseconds.
    pub updated_at_ms: u64,
    /// Whether this connector exposes a trustworthy upstream reconciliation operation.
    pub upstream_reconciliation_supported: bool,
}
/// Ordered owner/admin delivery page for one exact Bridge.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryPage {
    /// Items ordered by stable outbound identity.
    pub items: Vec<BridgeDelivery>,
    /// Exclusive cursor, equal to the last returned item when more exist.
    pub next_cursor: Option<String>,
}
/// Explicit operator recovery. Reconcile only refreshes expired local attempts
/// when the connector has no supported upstream reconciliation proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeDeliveryRecoveryAction {
    /// Queue the same original item using currently valid matching retained custody.
    Retry {
        /// Must be true when the previous send outcome is Unknown.
        accept_duplicate_risk: bool,
    },
    /// Stop this item while preserving its original receipt and all attempts.
    Abandon {},
    /// Recover expired local worker claims without inventing upstream delivery proof.
    Reconcile {},
}
/// Compare-and-swap recovery input. Preserve the complete request and key after response loss.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryRecoveryRequest {
    /// Original outbound item identity; cannot select a different recipient.
    pub item_id: String,
    /// Current item revision read by the manager.
    pub expected_revision: u64,
    /// Current Bridge revision read by the manager.
    pub expected_bridge_revision: u64,
    /// Explicit action and duplicate-risk choice.
    pub action: BridgeDeliveryRecoveryAction,
}
/// Permanent recovery receipt; repeated identical commands recover this original result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeliveryRecoveryReceipt {
    /// Stable recovery-command identity.
    pub id: String,
    /// Exact original Bridge identity.
    pub bridge_id: String,
    /// Digest binding the permanent key to the original complete input.
    pub request_sha256: String,
    /// Actual current human/API manager, never the Bridge owner impersonated.
    pub actor: ResourceActor,
    /// Original explicit operator action.
    pub action: BridgeDeliveryRecoveryAction,
    /// Safe item state immediately after the command committed.
    pub result: BridgeDelivery,
    /// Original recovery command time in Unix milliseconds.
    pub created_at_ms: u64,
}
