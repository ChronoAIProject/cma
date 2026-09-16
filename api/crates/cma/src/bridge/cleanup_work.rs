//! Explicit accepted jobs alone dispatch exact shared-child cleanup. A job
//! claim bounds scheduling; every effect still needs fresh original authority.
use std::sync::Arc;

use layer2_core::bridge::{
    allocation::{AllocationError, AllocationRecord},
    cleanup::{
        ChildCleanupHold as Hold, ChildCleanupProgress as Progress, CleanupError, CleanupJob,
    },
    BridgeService,
};

use super::{
    channel_scheduler::{ChannelWorkError as Error, ChannelWorkPort},
    provisioning::{reuse_cleanup::ChannelReuseCleanupAuthority, BridgeProvisioningAuthority},
};
use crate::{
    sandbox::{
        channel_reuse_cleanup::{
            ChannelReuseCleanupHold, ChannelReuseCleanupRequest, ChannelReuseRemovalPhase,
        },
        LiveAgentRegistry,
    },
    store::MongoBridgeAllocationStore,
};

#[async_trait::async_trait]
pub(crate) trait CleanupWorkAuthority: Send + Sync {
    async fn resolve(
        &self,
        record: &AllocationRecord,
    ) -> Result<ChannelReuseCleanupAuthority, Hold>;
}

pub(crate) struct ManagedCleanupWorkAuthority {
    pub definitions: BridgeService,
    pub authority: Option<Arc<BridgeProvisioningAuthority>>,
}

#[async_trait::async_trait]
impl CleanupWorkAuthority for ManagedCleanupWorkAuthority {
    async fn resolve(
        &self,
        record: &AllocationRecord,
    ) -> Result<ChannelReuseCleanupAuthority, Hold> {
        self.authority
            .as_ref()
            .ok_or(Hold::AuthorityUnavailable)?
            .resolve_reuse_cleanup(&self.definitions, record)
            .await
            .map_err(|_| Hold::AuthorityUnavailable)
    }
}

pub(crate) struct ChannelCleanupWork {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<dyn CleanupWorkAuthority>,
    pub adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    pub registry: Arc<LiveAgentRegistry>,
    pub deployment_id: String,
    pub holder: String,
}

#[async_trait::async_trait]
impl ChannelWorkPort for ChannelCleanupWork {
    fn kind(&self) -> super::operations::WorkKind {
        super::operations::WorkKind::ChildCleanup
    }
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, Error> {
        self.allocations
            .discover_cleanup_jobs(after)
            .await
            .map_err(error)
    }

    async fn run_one(&self, id: &str) -> Result<(), Error> {
        let (claimed, allocation, request) = self
            .allocations
            .claim_cleanup_job(id, &self.holder)
            .await
            .map_err(error)?;
        let claim = claimed.lease.as_ref().ok_or(Error::Corrupt)?;
        // A lost publication can leave progress behind actual physical work.
        // Reconcile proofs before requiring any currently available authority.
        let job = self
            .allocations
            .reconcile_cleanup_job_progress(id, claim)
            .await
            .map_err(error)?;
        if job.progress == Progress::Complete {
            return Ok(());
        }
        let (progress, holds) = match self.advance(&job, &allocation, &request).await {
            Ok(observation) => observation,
            Err(hold) => (job.progress, vec![hold]),
        };
        self.allocations
            .observe_cleanup_job(id, claim, progress, &holds)
            .await
            .map_err(error)?;
        if progress != Progress::Complete {
            super::operations::health::pending(
                super::operations::health::PendingStage::ChildCleanup,
                id,
                job.admission.receipt.accepted_at_ms,
                !holds.is_empty(),
            );
        }
        Ok(())
    }
}

impl ChannelCleanupWork {
    async fn advance(
        &self,
        job: &CleanupJob,
        allocation: &AllocationRecord,
        request: &ChannelReuseCleanupRequest,
    ) -> Result<(Progress, Vec<Hold>), Hold> {
        let mut permit = self.authority.resolve(allocation).await?;
        if permit.management().actor() != &job.admission.receipt.requested_by
            || permit.allocation() != allocation
        {
            return Err(Hold::AuthorityUnavailable);
        }
        // An expired scheduling claimant cannot begin a later runtime step.
        // Unknown transport outcomes retain C's same operation and exclusion.
        let instant = tokio::time::Instant::now();
        let now = mongodb::bson::DateTime::now().timestamp_millis();
        let claim = job.lease.as_ref().ok_or(Hold::EvidenceInconsistent)?;
        permit.bind_job(&job.admission.receipt.id, claim);
        job.require_claim(claim, now)
            .map_err(|_| Hold::EvidenceUnavailable)?;
        let remaining =
            u64::try_from(claim.expires_at_ms - now).map_err(|_| Hold::EvidenceUnavailable)?;
        permit.bound_deadline(instant + std::time::Duration::from_millis(remaining));
        permit.require_current().map_err(hold)?;
        if job.progress == Progress::Requested {
            self.allocations
                .close_channel_reuse_cleanup_admission(&permit, request)
                .await
                .map_err(hold)?;
            let observed = self
                .allocations
                .observe_channel_reuse_cleanup_quiescence(&permit, request)
                .await
                .map_err(hold)?;
            return Ok((Progress::AdmissionClosed, child_holds(&observed.holds)));
        }
        let observed = self
            .allocations
            .observe_channel_reuse_cleanup_quiescence(&permit, request)
            .await
            .map_err(hold)?;
        let holds = child_holds(&observed.holds);
        if !holds.is_empty() {
            return Ok((job.progress, holds));
        }
        let state = if job.progress == Progress::AdmissionClosed {
            self.allocations
                .prepare_channel_reuse_child_removal(&permit, request)
                .await
        } else {
            self.allocations
                .run_channel_reuse_child_removal(
                    &permit,
                    request,
                    self.adapter.as_ref(),
                    &self.registry,
                    &self.deployment_id,
                )
                .await
        }
        .map_err(hold)?;
        let progress = match state.phase {
            ChannelReuseRemovalPhase::Intent => Progress::RemovalStarted,
            ChannelReuseRemovalPhase::ProcessClosed => Progress::ProcessClosed,
            ChannelReuseRemovalPhase::HomeRemoved => Progress::HomeRemoved,
            ChannelReuseRemovalPhase::WorktreeRemoved => Progress::WorktreeRemoved,
            ChannelReuseRemovalPhase::Complete => Progress::Complete,
        };
        Ok((progress, Vec::new()))
    }
}

fn child_holds(holds: &[ChannelReuseCleanupHold]) -> Vec<Hold> {
    holds
        .iter()
        .filter_map(|hold| {
            Some(match hold {
                ChannelReuseCleanupHold::AgentBusy => Hold::AgentBusy,
                ChannelReuseCleanupHold::WorkspaceBusy => Hold::WorkspaceBusy,
                ChannelReuseCleanupHold::PendingCommand => Hold::PendingCommand,
                ChannelReuseCleanupHold::UncertainCommand => Hold::UncertainCommand,
                ChannelReuseCleanupHold::PendingReply => Hold::PendingReply,
                ChannelReuseCleanupHold::UncertainReply => Hold::UncertainReply,
                // This operation always retains the Workspace and all other children.
                ChannelReuseCleanupHold::OtherWorkspaceChildren => return None,
                ChannelReuseCleanupHold::InventoryLimit => Hold::InventoryLimit,
            })
        })
        .collect()
}

fn hold(error: AllocationError) -> Hold {
    match error {
        AllocationError::AuthorityUnavailable => Hold::AuthorityUnavailable,
        AllocationError::IdentityMismatch => Hold::RuntimeChanged,
        AllocationError::Corrupt => Hold::EvidenceInconsistent,
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
