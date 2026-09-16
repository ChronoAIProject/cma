//! Live, boot-correlated Workspace removal evidence. Historical receipts alone
//! do not prove that an archive has not restored the same Workspace directory.
use serde::{Deserialize, Serialize};

use crate::{
    AdapterError, Result, RuntimeStatus, RuntimeStatusRequest, WorkspaceRemoveReceipt,
    WorkspaceRemoveRequest,
};

pub const WORKSPACE_REMOVAL_PROTOCOL: u8 = 1;

/// A live supervisor's support for guarded completed replay and the ordered
/// restore client's final Workspace reconciliation. Never inferred from v3 labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemovalSupport {
    pub runtime: RuntimeStatus,
    pub protocol_version: u8,
}

impl WorkspaceRemovalSupport {
    pub fn validate_for(&self, request: &RuntimeStatusRequest) -> Result<()> {
        self.runtime.validate_for(request)?;
        if self.protocol_version != WORKSPACE_REMOVAL_PROTOCOL {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Transport authority is deliberately outside the canonical removal digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemovalRequest {
    pub runtime: RuntimeStatus,
    pub request: WorkspaceRemoveRequest,
}

impl WorkspaceRemovalRequest {
    pub fn validate(&self) -> Result<()> {
        self.runtime.validate_for(&self.runtime_request())?;
        if !self.runtime.installation_ready
            || !self.runtime.credential_ready
            || self.runtime.restore_pending
            || !self.request.has_valid_sha256()
            || self.request.operation_id
                != format!("workspace:{}:remove:v1", self.request.workspace_id)
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn runtime_request(&self) -> RuntimeStatusRequest {
        RuntimeStatusRequest {
            sandbox_id: self.runtime.sandbox_id.clone(),
            generation: self.runtime.generation,
            home_identity: self.runtime.home_identity.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRemovalOutcome {
    CurrentlyAbsent,
    /// The old canonical operation is complete, but a successor owns the name.
    /// This is a no-effect historical acknowledgement, never an absence proof.
    SuccessorPresent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRemovalObservation {
    pub runtime: RuntimeStatus,
    pub receipt: WorkspaceRemoveReceipt,
    pub outcome: WorkspaceRemovalOutcome,
}

impl WorkspaceRemovalObservation {
    pub fn validate_for(&self, request: &WorkspaceRemovalRequest) -> Result<()> {
        request.validate()?;
        self.runtime.validate_for(&request.runtime_request())?;
        if !same_runtime_writer(&self.runtime, &request.runtime)
            || self.receipt.operation_id != request.request.operation_id
            || self.receipt.request_sha256 != request.request.request_sha256
            || self.receipt.workspace_id != request.request.workspace_id
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Unrelated sibling process counts may change between observations; identity
/// and installation/credential/restore readiness must remain exact.
pub fn same_runtime_writer(left: &RuntimeStatus, right: &RuntimeStatus) -> bool {
    left.sandbox_id == right.sandbox_id
        && left.generation == right.generation
        && left.home_identity == right.home_identity
        && left.boot_id == right.boot_id
        && left.installation_ready == right.installation_ready
        && left.credential_ready == right.credential_ready
        && left.restore_pending == right.restore_pending
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("Workspace removal evidence does not prove the exact current runtime and canonical operation".into())
}

#[cfg(test)]
#[path = "workspace_removal_tests.rs"]
mod tests;
