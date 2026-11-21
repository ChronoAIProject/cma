//! Owner-initiated delivery never accepts a platform recipient or credential.
use crate::delivery::intents::DeliveryOutcome;
use cmaeg_core::{
    channel::SourceId,
    message::{OutboundMessage, RenderingReport},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareOutboundMessage {
    pub source_id: SourceId,
    /// Exact NyxID conversation UUID already configured on this source.
    pub route_id: String,
    pub message: OutboundMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutboundStatus {
    Prepared,
    Committed,
    Cancelled,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OutboundReceipt {
    pub delivery_id: String,
    pub source_id: SourceId,
    pub route_id: String,
    pub status: OutboundStatus,
    /// Last instant at which a prepared operation may commit.
    pub expires_at: i64,
    /// The idempotency key cannot be reused before this instant.
    pub retained_until: i64,
    pub rendering: RenderingReport,
    /// Committed without a recorded receipt is unknown; never retry its send.
    pub delivery: Option<DeliveryOutcome>,
}
