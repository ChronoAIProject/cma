//! Request-local original-human authority for reducing an exact reused child.
//! It grants no execution, runtime deletion, credential or capacity mutation.
use super::*;
use layer2_core::{
    bridge::{
        allocation::{AllocationError, AllocationState},
        BridgeManagementLease,
    },
    resource_access::{AccessPurpose, AuthorizedResource, ResourceCapability, ResourceTarget},
    sandbox::{SandboxStatus, SandboxWorkStep},
};

pub(crate) struct ChannelReuseCleanupAuthority {
    allocation: AllocationRecord,
    management: BridgeManagementLease,
    parent: AuthorizedResource,
    human: Arc<BridgeControlPlaneLease>,
    job: Option<(String, layer2_core::bridge::cleanup::CleanupJobClaim)>,
}

impl ChannelReuseCleanupAuthority {
    pub(crate) fn bind_job(
        &mut self,
        id: &str,
        claim: &layer2_core::bridge::cleanup::CleanupJobClaim,
    ) {
        self.job = Some((id.into(), claim.clone()));
    }
    pub(crate) fn job(&self) -> Option<(&str, &layer2_core::bridge::cleanup::CleanupJobClaim)> {
        self.job.as_ref().map(|(id, claim)| (id.as_str(), claim))
    }

    pub(crate) fn bound_deadline(&mut self, deadline: tokio::time::Instant) {
        self.management.bound_deadline(deadline);
    }
    #[cfg(test)]
    pub(crate) fn fixture(
        allocation: AllocationRecord,
        management: BridgeManagementLease,
        parent: AuthorizedResource,
        human: Arc<BridgeControlPlaneLease>,
    ) -> Self {
        Self {
            allocation,
            management,
            parent,
            human,
            job: None,
        }
    }

    pub(crate) fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub(crate) fn management(&self) -> &BridgeManagementLease {
        &self.management
    }
    pub(crate) fn parent(&self) -> &layer2_core::resource_access::ResourceAccessSnapshot {
        &self.parent.snapshot
    }
    pub(crate) fn human(&self) -> &BridgeControlPlaneLease {
        &self.human
    }

    pub(crate) fn require_current(&self) -> Result<(), AllocationError> {
        self.allocation.validate()?;
        self.management
            .require_current()
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.parent
            .decision
            .require(ResourceCapability::ManageSandbox)
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let record = &self.allocation;
        let AllocationPlacement::ReuseParent {
            target,
            profiles,
            parent,
        } = &record.placement
        else {
            return Err(AllocationError::IdentityMismatch);
        };
        let bridge = self.management.record();
        let sandbox = &self.parent.snapshot.lineage.sandbox;
        // Parent identity and pinned profile snapshots are evidence. Cleanup
        // never fetches retired profile definitions or renews accepted work.
        parent.require(&record.config, profiles, &self.parent.snapshot)?;
        let scope = self
            .parent
            .snapshot
            .scope(
                &record
                    .config
                    .placement
                    .reused_parent()
                    .ok_or(AllocationError::IdentityMismatch)?,
            )
            .map_err(|_| AllocationError::IdentityMismatch)?;
        if record.state != AllocationState::Retired
            || record.receipt.is_none()
            || !record.effect_started
            || record.lease.is_some()
            || bridge.id != record.identity.bridge_id
            || bridge.owner != record.owner
            || bridge
                .pinned(&record.pin)
                .map_err(|_| AllocationError::IdentityMismatch)?
                .config
                != record.config
            || scope.owner != record.owner
            || self.human.owner() != &record.owner
            || self.human.bridge_id() != &bridge.id
            || self.human.connector() != &record.config.connector
            || self.management.actor() != &self.human.actor_context().actor
            || self.management.actor() != &profiles.approved_by
            || tokio::time::Instant::now() >= self.human.deadline()
            || sandbox.id != target.sandbox_id
            || sandbox.status != SandboxStatus::Active
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.lifecycle.background_checkpoint.is_some()
            || sandbox.work.lease.is_some()
            || sandbox.work.execution.is_some()
            || !matches!(
                sandbox.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || sandbox.runtime.as_ref().is_none_or(|runtime| {
                runtime.provision_generation.is_none() || runtime.boot_id.is_none()
            })
        {
            return Err(AllocationError::AuthorityUnavailable);
        }
        Ok(())
    }
}

impl BridgeProvisioningAuthority {
    pub(crate) async fn resolve_reuse_cleanup(
        &self,
        definitions: &BridgeService,
        record: &AllocationRecord,
    ) -> Result<ChannelReuseCleanupAuthority, BridgeError> {
        record.validate().map_err(allocation_error)?;
        if record.state != AllocationState::Retired
            || !matches!(record.placement, AllocationPlacement::ReuseParent { .. })
        {
            return Err(BridgeError::Conflict);
        }
        let key = self
            .credentials
            .resolve(&record.owner, &record.config)
            .await?;
        let bridge = self
            .credentials
            .bridges
            .bridge(&record.identity.bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let mut management = definitions
            .management_lease(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &bridge.id,
                bridge.revision,
            )
            .await?;
        management.bound_deadline(key.deadline);
        // A whole-parent capability is required even for one child. No narrow
        // Agent share can provide this cleanup authority.
        let target = record
            .config
            .placement
            .reused_parent()
            .ok_or(BridgeError::Conflict)?;
        let root = ResourceTarget::Sandbox {
            sandbox_id: record.placement.target().sandbox_id.clone(),
        };
        let whole = self
            .dependencies
            .resource_access
            .authorize(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &root,
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .map_err(|_| BridgeError::ConnectionRequired(BridgeSetupBlock::IncompatibleTarget))?;
        let parent = self
            .dependencies
            .resource_access
            .authorize(
                key.control_plane.actor_context(),
                Some(key.control_plane.credential()),
                &target,
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .map_err(|_| BridgeError::ConnectionRequired(BridgeSetupBlock::IncompatibleTarget))?;
        if whole.snapshot.lineage.sandbox != parent.snapshot.lineage.sandbox
            || whole.snapshot.policy != parent.snapshot.policy
            || whole.snapshot.shares != parent.snapshot.shares
        {
            return Err(BridgeError::Conflict);
        }
        if let Some(deadline) = whole
            .decision
            .revalidation_deadline(ResourceCapability::ManageSandbox)
        {
            management.bound_deadline(deadline);
        }
        self.dependencies
            .resources
            .require_ordinary_reuse_parent(&parent.snapshot.lineage.sandbox)
            .await
            .map_err(allocation_error)?;
        self.credentials.revalidate(&key).await?;
        definitions.validate_management(&management).await?;
        let permit = ChannelReuseCleanupAuthority {
            allocation: record.clone(),
            management,
            parent,
            human: Arc::new(key.control_plane),
            job: None,
        };
        permit.require_current().map_err(allocation_error)?;
        Ok(permit)
    }
}
