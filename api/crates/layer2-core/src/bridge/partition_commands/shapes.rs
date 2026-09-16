use super::*;
use crate::{
    bridge::{allocation::AllocationState, BridgeId},
    resource_access::ResourceActor,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionCommandAction {
    Reset,
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionCommandRequest {
    pub allocation_id: String,
    pub action: PartitionCommandAction,
    pub expected_bridge_revision: u64,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionCommandResult {
    pub id: String,
    pub generation: u64,
    pub revision: u64,
    pub state: AllocationState,
}
impl From<&AllocationRecord> for PartitionCommandResult {
    fn from(record: &AllocationRecord) -> Self {
        Self {
            id: record.id.clone(),
            generation: record.identity.generation,
            revision: record.revision,
            state: record.state,
        }
    }
}

/// Permanent response for the exact original command, even after later resets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionCommandReceipt {
    pub id: String,
    pub bridge_id: BridgeId,
    pub request_sha256: String,
    pub actor: ResourceActor,
    pub action: PartitionCommandAction,
    pub expected_bridge_revision: u64,
    pub allocation_id: String,
    pub generation: u64,
    pub previous_revision: u64,
    pub result: PartitionCommandResult,
    pub successor: Option<PartitionCommandResult>,
    pub created_at_ms: i64,
}
impl PartitionCommandReceipt {
    pub fn validate(&self) -> Result<(), PartitionCommandError> {
        let valid_revision = |v| v > 0 && v <= i64::MAX as u64;
        if !self.id.strip_prefix("bpc_").is_some_and(digest)
            || !digest(&self.request_sha256)
            || !allocation_id(&self.allocation_id)
            || !(1..=256).contains(&self.generation)
            || !valid_revision(self.expected_bridge_revision)
            || !valid_revision(self.previous_revision)
            || self.result.id != self.allocation_id
            || self.result.generation != self.generation
            || self.result.revision
                != self
                    .previous_revision
                    .checked_add(1)
                    .ok_or(PartitionCommandError::Corrupt)?
            || !valid_revision(self.result.revision)
            || self.result.state != AllocationState::Retired
            || self.created_at_ms < 0
        {
            return Err(PartitionCommandError::Corrupt);
        }
        match (self.action, &self.successor) {
            (PartitionCommandAction::Detach, None) => {}
            (PartitionCommandAction::Reset, Some(next))
                if allocation_id(&next.id)
                    && next.id != self.allocation_id
                    && next.generation == self.generation + 1
                    && next.generation <= 256
                    && next.revision == 1
                    && matches!(
                        next.state,
                        AllocationState::Reserved | AllocationState::Ready
                    ) => {}
            _ => return Err(PartitionCommandError::Corrupt),
        }
        let request = PartitionCommandRequest {
            allocation_id: self.allocation_id.clone(),
            action: self.action,
            expected_bridge_revision: self.expected_bridge_revision,
            expected_revision: self.previous_revision,
        };
        if command_hash(&request)? != self.request_sha256 {
            return Err(PartitionCommandError::Corrupt);
        }
        Ok(())
    }
}
