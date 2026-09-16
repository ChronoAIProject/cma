//! Inert Workspace acceptance fixture. No Mongo proof, capacity mutation or runtime effect.
use super::*;

pub(super) fn child_receipt_id() -> String {
    format!("bclj_{}", "9".repeat(64))
}
impl Memory {
    pub(super) fn read_workspace(
        &self,
        access: &CleanupAccess,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        access.require_current()?;
        self.available()?;
        let runtime_supported = self.runtime_supported.load(Ordering::SeqCst);
        if let Some((_, prior)) = self.workspace_state.lock().unwrap().as_ref() {
            let mut status = prior.clone();
            status.runtime_supported = runtime_supported;
            return Ok(status);
        }
        let allocation = access.allocation();
        let owned = matches!(
            allocation.config.placement,
            BridgePlacementPolicy::WorkspacePerPartition { .. }
        );
        let child_complete = self.child_complete.load(Ordering::SeqCst);
        let mut holds = vec![];
        if !owned {
            holds.push(WorkspaceCleanupHold::WorkspaceNotOwned);
        }
        if !child_complete {
            holds.push(WorkspaceCleanupHold::ChildCleanupIncomplete);
        }
        if !runtime_supported {
            holds.push(WorkspaceCleanupHold::RuntimeUnsupported);
        }
        Ok(WorkspaceCleanupStatus {
            bridge_id: allocation.identity.bridge_id.clone(),
            allocation_id: allocation.id.clone(),
            generation: allocation.identity.generation,
            allocation_revision: allocation.revision,
            target: allocation.placement.target().clone(),
            workspace_revision: owned.then_some(3),
            child_cleanup_receipt_id: child_complete.then(child_receipt_id),
            eligible: holds.is_empty(),
            runtime_supported,
            receipt: None,
            progress: WorkspaceCleanupProgress::NotRequested,
            holds,
            observed_at_ms: Some(900),
            workspace_capacity_released: false,
            parent_retained: true,
            history_retained: true,
        })
    }
    pub(super) fn recover_workspace(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<WorkspaceCleanupStatus>, CleanupError> {
        access.require_current()?;
        self.available()?;
        let prior = self.workspace_state.lock().unwrap().clone();
        let Some((prior_digest, status)) = prior else {
            return Ok(None);
        };
        let receipt = status.receipt.as_ref().ok_or(CleanupError::Corrupt)?;
        if receipt.id != id {
            return Ok(None);
        }
        if prior_digest != digest || receipt.requested_by != *access.management().actor() {
            return Err(CleanupError::Conflict);
        }
        self.read_workspace(access).map(Some)
    }
    pub(super) fn submit_workspace(
        &self,
        write: WorkspaceCleanupWrite,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        write.require_current()?;
        let mut status = self.read_workspace(write.access())?;
        if !status.eligible
            || status.workspace_revision != Some(write.request().expected_workspace_revision)
            || status.child_cleanup_receipt_id.as_ref()
                != Some(&write.request().child_cleanup_receipt_id)
        {
            return Err(CleanupError::Conflict);
        }
        let mut state = self.workspace_state.lock().unwrap();
        if state.is_some() {
            return Err(CleanupError::Conflict);
        }
        status.receipt = Some(WorkspaceCleanupReceipt {
            id: write.receipt_id().into(),
            bridge_id: status.bridge_id.clone(),
            allocation_id: status.allocation_id.clone(),
            generation: status.generation,
            target: status.target.clone(),
            request: write.request().clone(),
            requested_by: write.access().management().actor().clone(),
            accepted_at_ms: 1000,
        });
        status.progress = WorkspaceCleanupProgress::Requested;
        status.eligible = false;
        status.observed_at_ms = Some(1000);
        *state = Some((write.request_sha256().into(), status.clone()));
        Ok(status)
    }
}
