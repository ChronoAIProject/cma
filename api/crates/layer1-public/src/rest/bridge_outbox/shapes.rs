#![allow(dead_code)]
use super::super::resource_access_shapes::ResourceActorView;
use layer2_core::bridge::outbox::management::OutboxRecoveryAction;
use serde::Deserialize;
use utoipa::ToSchema;
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutboxStatusShape {
    Pending,
    Claimed,
    Sending,
    RetryWait,
    Sent,
    Rejected,
    Unknown,
    Abandoned,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryFailureShape {
    CredentialExpired,
    CredentialRevoked,
    BotInactive,
    RouteUnavailable,
    CapabilityUnsupported,
    RateLimited,
    RequestRejected,
    ConnectUnavailable,
    ProviderFailure,
    ResponseLost,
    WorkerLost,
    ReceiptInvalid,
}
#[derive(ToSchema)]
pub(crate) struct OutboxAttemptShape {
    id: String,
    number: u32,
    started_at_ms: i64,
    status: OutboxStatusShape,
    failure: Option<DeliveryFailureShape>,
    retained_key: bool,
}
#[derive(ToSchema)]
pub(crate) struct OutboxItemShape {
    id: String,
    bridge_id: String,
    config_revision: u64,
    receipt_id: String,
    response_id: String,
    revision: u64,
    sequence: u64,
    chunk_index: u32,
    chunk_count: u32,
    status: OutboxStatusShape,
    failure: Option<DeliveryFailureShape>,
    duplicate_risk: bool,
    next_retry_at_ms: Option<i64>,
    attempts: Vec<OutboxAttemptShape>,
    created_at_ms: i64,
    updated_at_ms: i64,
    upstream_reconciliation_supported: bool,
}
#[derive(ToSchema)]
pub(crate) struct OutboxPageShape {
    items: Vec<OutboxItemShape>,
    next_cursor: Option<String>,
}
#[derive(ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum OutboxRecoveryActionShape {
    Retry { accept_duplicate_risk: bool },
    Abandon,
    Reconcile,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryRequest {
    pub item_id: String,
    pub expected_revision: u64,
    pub expected_bridge_revision: u64,
    #[schema(value_type=OutboxRecoveryActionShape)]
    pub action: OutboxRecoveryAction,
}
#[derive(ToSchema)]
pub(crate) struct OutboxRecoveryShape {
    id: String,
    bridge_id: String,
    request_sha256: String,
    actor: ResourceActorView,
    action: OutboxRecoveryActionShape,
    result: OutboxItemShape,
    created_at_ms: i64,
}
