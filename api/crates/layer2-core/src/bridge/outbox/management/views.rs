use super::super::{DeliveryFailure, DeliveryState, OutboxItem};
use crate::{bridge::BridgeId, resource_access::ResourceActor};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    Pending,
    Claimed,
    Sending,
    RetryWait,
    Sent,
    Rejected,
    Unknown,
    Abandoned,
}
impl From<&DeliveryState> for OutboxStatus {
    fn from(value: &DeliveryState) -> Self {
        match value {
            DeliveryState::Pending => Self::Pending,
            DeliveryState::Claimed { .. } => Self::Claimed,
            DeliveryState::Sending { .. } => Self::Sending,
            DeliveryState::RetryWait { .. } => Self::RetryWait,
            DeliveryState::Sent { .. } => Self::Sent,
            DeliveryState::Rejected { .. } => Self::Rejected,
            DeliveryState::Unknown { .. } => Self::Unknown,
            DeliveryState::Abandoned { .. } => Self::Abandoned,
        }
    }
}
fn failure(state: &DeliveryState) -> Option<DeliveryFailure> {
    match state {
        DeliveryState::RetryWait { reason, .. }
        | DeliveryState::Rejected { reason }
        | DeliveryState::Unknown { reason, .. } => Some(*reason),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxAttemptView {
    pub id: String,
    pub number: u32,
    pub started_at_ms: i64,
    pub status: OutboxStatus,
    pub failure: Option<DeliveryFailure>,
    pub retained_key: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxItemView {
    pub id: String,
    pub bridge_id: BridgeId,
    pub config_revision: u64,
    pub receipt_id: String,
    pub response_id: String,
    pub revision: u64,
    pub sequence: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub status: OutboxStatus,
    pub failure: Option<DeliveryFailure>,
    pub duplicate_risk: bool,
    pub next_retry_at_ms: Option<i64>,
    pub attempts: Vec<OutboxAttemptView>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Pinned provider has no trustworthy arbitrary-send reconciliation API.
    pub upstream_reconciliation_supported: bool,
}
impl From<&OutboxItem> for OutboxItemView {
    fn from(item: &OutboxItem) -> Self {
        Self {
            id: item.id.clone(),
            bridge_id: item.source.pin().bridge_id.clone(),
            config_revision: item.source.pin().config_revision,
            receipt_id: item.tool_invocation_id.clone(),
            response_id: item.source.response_id().to_string(),
            revision: item.revision,
            sequence: item.sequence,
            chunk_index: item.chunk_index,
            chunk_count: item.chunk_count,
            status: (&item.state).into(),
            failure: failure(&item.state),
            duplicate_risk: item.duplicate_risk,
            next_retry_at_ms: match item.state {
                DeliveryState::RetryWait { retry_at_ms, .. } => Some(retry_at_ms),
                _ => None,
            },
            attempts: item
                .attempts
                .iter()
                .map(|attempt| OutboxAttemptView {
                    id: attempt.id.clone(),
                    number: attempt.number,
                    started_at_ms: attempt.started_at_ms,
                    status: (&attempt.state).into(),
                    failure: failure(&attempt.state),
                    retained_key: !attempt.credential.is_reply_token(),
                })
                .collect(),
            created_at_ms: item.created_at_ms,
            updated_at_ms: item.updated_at_ms,
            upstream_reconciliation_supported: false,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct OutboxPage {
    pub items: Vec<OutboxItemView>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutboxRecoveryAction {
    Retry { accept_duplicate_risk: bool },
    Abandon {},
    Reconcile {},
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxRecoveryRequest {
    pub item_id: String,
    pub expected_revision: u64,
    pub expected_bridge_revision: u64,
    pub action: OutboxRecoveryAction,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxRecoveryReceipt {
    pub id: String,
    pub bridge_id: BridgeId,
    pub request_sha256: String,
    pub actor: ResourceActor,
    pub action: OutboxRecoveryAction,
    pub result: OutboxItemView,
    pub created_at_ms: i64,
}
