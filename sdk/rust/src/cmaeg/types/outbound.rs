use super::{DeliveryOutcome, OutboundMessage, RenderingReport, SourceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// An owner-reviewed message addressed to an already saved relay destination.
pub struct PrepareOutboundMessage {
    /// Exact source identity in the persisted channel definition.
    pub source_id: SourceId,
    /// Exact configured NyxID conversation UUID; never an arbitrary recipient.
    pub route_id: String,
    /// Typed shared message; commit must supply this same content.
    pub message: OutboundMessage,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Durable operation state, separate from platform delivery outcome.
pub enum OutboundStatus {
    /// Admitted without an external send; awaits explicit commit or cancellation.
    Prepared,
    /// One send attempt claimed; never automatically repeated.
    Committed,
    /// Cancelled before any send attempt was claimed.
    Cancelled,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
/// Durable owner-bound reference and observed platform outcome.
pub struct OutboundReceipt {
    /// Stable operation identifier used for explicit status checks.
    pub delivery_id: String,
    /// Exact source identity in the persisted channel definition.
    pub source_id: SourceId,
    /// Exact configured NyxID conversation UUID; never an arbitrary recipient.
    pub route_id: String,
    /// Current durable operation state.
    pub status: OutboundStatus,
    /// Unix timestamp after which a prepared operation cannot commit.
    pub expires_at: i64,
    /// Immutable Unix timestamp ending idempotency retention.
    pub retained_until: i64,
    /// Explicit rendering decisions for the selected platform.
    pub rendering: RenderingReport,
    #[serde(default)]
    /// Platform outcome; committed uncertainty must never trigger a resend.
    pub delivery: Option<DeliveryOutcome>,
}
