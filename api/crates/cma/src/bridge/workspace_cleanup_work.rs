//! The dedicated Workspace worker never invokes the generic WorkspaceWorker.
use super::{
    channel_scheduler::{ChannelWorkError as Error, ChannelWorkPort},
    cleanup_work::CleanupWorkAuthority,
    provisioning::workspace_cleanup::WorkspaceCleanupAuthority,
};
use crate::store::MongoBridgeAllocationStore;
use layer2_core::bridge::{
    allocation::AllocationRecord,
    cleanup::{
        CleanupError, WorkspaceCleanupHold as Hold, WorkspaceCleanupJob,
        WorkspaceCleanupProgress as Progress,
    },
};
use std::sync::Arc;

pub(crate) struct WorkspaceCleanupWork {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<dyn CleanupWorkAuthority>,
    pub adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    pub deployment_id: String,
    pub holder: String,
}
#[async_trait::async_trait]
impl ChannelWorkPort for WorkspaceCleanupWork {
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, Error> {
        self.allocations
            .discover_workspace_cleanup_jobs(after)
            .await
            .map_err(error)
    }
    async fn run_one(&self, id: &str) -> Result<(), Error> {
        let (claimed, allocation, _) = self
            .allocations
            .claim_workspace_cleanup_job(id, &self.holder)
            .await
            .map_err(error)?;
        let claim = claimed.lease.as_ref().ok_or(Error::Corrupt)?;
        let job = self
            .allocations
            .reconcile_workspace_cleanup_job(id, claim)
            .await
            .map_err(error)?;
        if job.progress == Progress::Complete {
            return Ok(());
        }
        let (progress, holds) = match self.advance(&job, &allocation).await {
            Ok(progress) => (progress, Vec::new()),
            Err(hold) => (job.progress, vec![hold]),
        };
        self.allocations
            .observe_workspace_cleanup_job(id, claim, progress, &holds)
            .await
            .map_err(error)?;
        Ok(())
    }
}
impl WorkspaceCleanupWork {
    async fn advance(
        &self,
        job: &WorkspaceCleanupJob,
        allocation: &AllocationRecord,
    ) -> Result<Progress, Hold> {
        let common = self
            .authority
            .resolve(allocation)
            .await
            .map_err(|_| Hold::AuthorityUnavailable)?;
        if common.management().actor() != &job.admission.receipt.requested_by
            || common.allocation() != allocation
        {
            return Err(Hold::AuthorityUnavailable);
        }
        let claim = job.lease.as_ref().ok_or(Hold::EvidenceInconsistent)?;
        let permit = WorkspaceCleanupAuthority::bind(common, &job.admission.receipt.id, claim)
            .map_err(|_| Hold::AuthorityUnavailable)?;
        let holds = self
            .allocations
            .workspace_cleanup_holds(&permit)
            .await
            .map_err(hold)?;
        if let Some(hold) = holds.first() {
            return Err(*hold);
        }
        let sandbox = &permit.common().parent().lineage.sandbox;
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            sandbox,
            &self.deployment_id,
        )
        .await
        .map_err(|_| Hold::RuntimeChanged)?;
        let support = self
            .adapter
            .workspace_removal_support(&runtime)
            .await
            .map_err(|_| Hold::RuntimeUnsupported)?;
        permit
            .require_current()
            .map_err(|_| Hold::AuthorityUnavailable)?;
        let state = self
            .allocations
            .step_workspace_cleanup(&permit, &support, None)
            .await
            .map_err(hold)?;
        // Persisting intent and issuing runtime IO are separate scheduler passes.
        if job.progress != Progress::RemovalStarted {
            return Ok(state.progress);
        }
        let request = state.removal.as_ref().ok_or(Hold::EvidenceInconsistent)?;
        permit
            .require_current()
            .map_err(|_| Hold::AuthorityUnavailable)?;
        let observation = self
            .adapter
            .reconcile_workspace_removal(&runtime, &request.runtime, &request.request)
            .await
            .map_err(|_| Hold::EvidenceUnavailable)?;
        permit
            .require_current()
            .map_err(|_| Hold::AuthorityUnavailable)?;
        observation
            .validate_for(request)
            .map_err(|_| Hold::EvidenceInconsistent)?;
        if observation.outcome != layer3_adapter::WorkspaceRemovalOutcome::CurrentlyAbsent {
            return Err(Hold::EvidenceInconsistent);
        }
        Ok(self
            .allocations
            .step_workspace_cleanup(&permit, &support, Some(&observation))
            .await
            .map_err(hold)?
            .progress)
    }
}
fn hold(error: CleanupError) -> Hold {
    match error {
        CleanupError::Corrupt | CleanupError::Invalid => Hold::EvidenceInconsistent,
        CleanupError::OriginalActorRequired => Hold::AuthorityUnavailable,
        _ => Hold::EvidenceUnavailable,
    }
}
fn error(error: CleanupError) -> Error {
    match error {
        CleanupError::Conflict | CleanupError::NotFound => Error::Changed,
        CleanupError::Corrupt | CleanupError::Invalid => Error::Corrupt,
        CleanupError::OriginalActorRequired => Error::Suspended,
        CleanupError::Unavailable | CleanupError::Bridge(_) => Error::Unavailable,
    }
}
