use super::*;
use layer2_core::bridge::{
    allocation::{AllocationPlacement, AllocationState},
    BridgePlacementPolicy,
};

impl MongoBridgeAllocationStore {
    pub(crate) async fn managed_workspace_cleanup_status(
        &self,
        access: &CleanupAccess,
        supported: bool,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        self.workspace_cleanup_read(access, supported, None)
            .await?
            .ok_or(CleanupError::Corrupt)
    }
    pub(crate) async fn recover_workspace_cleanup_job(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<WorkspaceCleanupStatus>, CleanupError> {
        self.workspace_cleanup_read(access, false, Some((id, digest)))
            .await
    }
    async fn workspace_cleanup_read(
        &self,
        access: &CleanupAccess,
        supported: bool,
        recovery: Option<(&str, &str)>,
    ) -> Result<Option<WorkspaceCleanupStatus>, CleanupError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            access.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CleanupError::Unavailable)?;
            let result = async {
                let row = self.cleanup_job_access(access, &mut session).await?;
                let database = self.bridges.client.database(&self.records.namespace().db);
                let retained = proof::retained(
                    &database,
                    access.allocation(),
                    &access.parent().lineage.sandbox,
                    &mut session,
                )
                .await?;
                let status = match (retained, recovery) {
                    (Some((job, _, workspace)), recovery) => {
                        if let Some((id, digest)) = recovery {
                            if job.admission.receipt.id != id
                                || job.admission.request_sha256 != digest
                                || job.admission.receipt.requested_by
                                    != *access.management().actor()
                            {
                                return Err(JobError::Core(CleanupError::Conflict));
                            }
                        }
                        Some(job.status(workspace.revision, supported)?)
                    }
                    (None, Some(_)) => None,
                    (None, None) => Some(
                        self.unrequested_workspace_status(access, &row, supported, &mut session)
                            .await?,
                    ),
                };
                if let Some(status) = &status {
                    access.require_workspace_status(status)?;
                }
                access.require_current()?;
                Ok::<_, JobError>(status)
            }
            .await;
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    if error.retryable() {
                        continue;
                    }
                    return Err(error.core());
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    access.require_current()?;
                    return Ok(result);
                }
                CommitOutcome::Retry(_) => continue,
                _ => return Err(CleanupError::Unavailable),
            }
        }
        Err(CleanupError::Unavailable)
    }

    async fn unrequested_workspace_status(
        &self,
        access: &CleanupAccess,
        row: &Document,
        supported: bool,
        session: &mut ClientSession,
    ) -> Result<WorkspaceCleanupStatus, JobError> {
        let allocation = access.allocation();
        let database = self.bridges.client.database(&self.records.namespace().db);
        let mut status = WorkspaceCleanupStatus {
            bridge_id: allocation.identity.bridge_id.clone(),
            allocation_id: allocation.id.clone(),
            generation: allocation.identity.generation,
            allocation_revision: allocation.revision,
            target: allocation.placement.target().clone(),
            workspace_revision: None,
            child_cleanup_receipt_id: None,
            eligible: false,
            runtime_supported: supported,
            receipt: None,
            progress: WorkspaceCleanupProgress::NotRequested,
            holds: Vec::new(),
            observed_at_ms: Some(mongodb::bson::DateTime::now().timestamp_millis()),
            workspace_capacity_released: false,
            parent_retained: true,
            history_retained: true,
        };
        if access
            .allocation()
            .placement
            .profiles()
            .is_some_and(|profiles| &profiles.approved_by != access.management().actor())
        {
            status
                .holds
                .push(WorkspaceCleanupHold::AuthorityUnavailable);
        }
        if access.parent().lineage.sandbox.status != layer2_core::sandbox::SandboxStatus::Active {
            status.holds.push(WorkspaceCleanupHold::ParentUnavailable);
        }
        if !supported {
            status.holds.push(WorkspaceCleanupHold::RuntimeUnsupported);
        }
        if allocation.state != AllocationState::Retired {
            status
                .holds
                .push(WorkspaceCleanupHold::AllocationNotRetired);
        }
        let owned = matches!(&allocation.placement, AllocationPlacement::ReuseParent {parent, ..} if parent.workspace_created_at.is_none())
            && matches!(
                allocation.config.placement,
                BridgePlacementPolicy::WorkspacePerPartition { .. }
            );
        if !owned {
            status.holds.push(WorkspaceCleanupHold::WorkspaceNotOwned);
        }
        if owned && allocation.state == AllocationState::Retired {
            let child_job = self
                .require_cleanup_job(&database, allocation, row, session)
                .await?;
            if let Some(job) =
                child_job.filter(|job| job.progress == ChildCleanupProgress::Complete)
            {
                let (_, child) =
                    child::complete(&database, allocation, &job.admission.receipt.id, session)
                        .await?;
                let (workspace, holds) = ownership::require(
                    &database,
                    allocation,
                    &access.parent().lineage.sandbox,
                    &child,
                    false,
                    session,
                )
                .await?;
                status.workspace_revision = Some(workspace.revision);
                status.child_cleanup_receipt_id = Some(job.admission.receipt.id);
                status.holds.extend(holds);
            } else {
                status
                    .holds
                    .push(WorkspaceCleanupHold::ChildCleanupIncomplete);
            }
        }
        status.holds.sort();
        status.holds.dedup();
        status.eligible = status.holds.is_empty();
        Ok(status)
    }
}
