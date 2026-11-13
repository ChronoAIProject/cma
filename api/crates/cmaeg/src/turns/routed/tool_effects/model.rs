use super::super::{storage, ExecutionError};
use crate::{
    delivery::retained_receipt::StoredReceipt,
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
};
use cmaeg_core::{message::OutboundMessage, seal::ReplyTargetPayload};
use serde::{Deserialize, Serialize};

pub(super) const KIND: &str = "routed_delayed_reply_v1";
pub(super) const CHUNK_BYTES: usize = 24 * 1024;
pub(super) const MAX_BYTES: usize = 192 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Payload {
    pub capability: ReplyTargetPayload,
    pub caller: [u8; 32],
    pub message: OutboundMessage,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Root {
    pub kind: String,
    pub id: String,
    pub identity: EventIdentity,
    pub admission: String,
    pub target: String,
    pub request_hash: String,
    pub retained_until: i64,
    pub operation: String,
    pub content_hash: String,
    pub payload_hash: String,
    pub bytes: usize,
    pub expires_at: i64,
    pub state: State,
    #[serde(skip)]
    pub version: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum State {
    Queued,
    Sending,
    Settled { receipt: StoredReceipt },
    Unknown { receipt: Option<StoredReceipt> },
}
impl Root {
    pub fn validate(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
    ) -> Result<(), ExecutionError> {
        if self.kind != KIND
            || self.id != work_id(&self.operation)
            || self.id == target.id
            || self.identity != plan.identity
            || self.admission != plan.admission_id
            || self.target != target.id
            || self.request_hash != plan.request_hash
            || self.retained_until != plan.retained_until
            || self.bytes == 0
            || self.bytes > MAX_BYTES
            || self.version == 0
            || self.expires_at <= 0
            || self.expires_at > self.retained_until
            || matches!(&self.state, State::Settled { receipt } if unresolved(receipt))
            || matches!(&self.state, State::Unknown { receipt: Some(receipt) } if !unresolved(receipt))
            || [&self.operation, &self.content_hash, &self.payload_hash]
                .into_iter()
                .any(|v| !super::super::tool_operation::valid_hash(v))
        {
            return Err(ExecutionError::Corrupt);
        }
        Ok(())
    }
    pub fn chunk_id(&self, index: usize) -> String {
        storage::identifier(&(KIND, &self.id, &self.payload_hash, index))
    }
    pub fn active(&self) -> bool {
        !matches!(self.state, State::Settled { .. })
    }
}
pub(super) fn work_id(operation: &str) -> String {
    storage::identifier(&(KIND, operation))
}
pub(super) fn operation(
    target: &StoredTarget,
    payload: &Payload,
    key: &str,
) -> Result<String, ExecutionError> {
    let capability = &payload.capability;
    // Reminting a capability and randomized token encryption do not create a
    // second operation. Exact caller/source/consumer scope remains bound.
    Ok(storage::hash(&storage::bytes(&(
        KIND,
        &target.id,
        &capability.provider,
        &capability.binding_id,
        &capability.agent_key_hash,
        &capability.event,
        key,
    ))?))
}

// A failed aggregate with confirmed earlier parts is partial delivery, even if
// a transport classified its final failure more narrowly than Unknown.
pub(super) fn unresolved(receipt: &StoredReceipt) -> bool {
    use crate::delivery::intents::DeliveryOutcome;
    matches!(receipt.delivery, DeliveryOutcome::Unknown { .. })
        || (receipt.has_parts() && !matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }))
}
