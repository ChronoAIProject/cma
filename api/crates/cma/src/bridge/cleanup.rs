//! Connect explicit core cleanup intent to retained original-actor authority.
use super::{provisioning::BridgeProvisioningAuthority, ManagedBridgeCredentials};
use crate::store::MongoBridgeAllocationStore;
use layer2_core::bridge::{
    allocation::AllocationRecord, cleanup::*, BridgeManagementLease, BridgeService,
};
use std::sync::Arc;

pub(crate) struct ManagedBridgeCleanup {
    definitions: BridgeService,
    allocations: Arc<MongoBridgeAllocationStore>,
    authority: Option<Arc<BridgeProvisioningAuthority>>,
}
impl ManagedBridgeCleanup {
    pub(crate) fn new(
        definitions: BridgeService,
        allocations: Arc<MongoBridgeAllocationStore>,
        credentials: Option<Arc<ManagedBridgeCredentials>>,
    ) -> Self {
        let authority = credentials.and_then(|credentials| {
            credentials.provisioning.clone().map(|dependencies| {
                Arc::new(BridgeProvisioningAuthority::new(credentials, dependencies))
            })
        });
        Self {
            definitions,
            allocations,
            authority,
        }
    }
}

#[async_trait::async_trait]
impl BridgeCleanupStore for ManagedBridgeCleanup {
    async fn allocations(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationRecord>, CleanupError> {
        self.allocations
            .managed_cleanup_allocations(manager, after, limit)
            .await
    }
    async fn allocation(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<AllocationRecord>, CleanupError> {
        self.allocations
            .managed_cleanup_allocation(manager, id)
            .await
    }
    async fn status(&self, access: &CleanupAccess) -> Result<ChildCleanupStatus, CleanupError> {
        self.allocations.managed_cleanup_status(access).await
    }
    async fn recover(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<ChildCleanupStatus>, CleanupError> {
        self.allocations
            .recover_cleanup_job(access, id, digest)
            .await
    }
    async fn submit(&self, write: CleanupWrite) -> Result<ChildCleanupStatus, CleanupError> {
        if let Some(status) = self
            .allocations
            .recover_cleanup_job(write.access(), write.receipt_id(), write.request_sha256())
            .await?
        {
            return Ok(status);
        }
        write.require_current()?;
        let authority = self.authority.as_ref().ok_or(CleanupError::Unavailable)?;
        let mut permit = authority
            .resolve_reuse_cleanup(&self.definitions, write.access().allocation())
            .await?;
        permit.bound_deadline(write.access().management().deadline());
        if permit.management().actor() != write.access().management().actor() {
            return Err(CleanupError::OriginalActorRequired);
        }
        if permit.management().record() != write.access().management().record() {
            return Err(CleanupError::Conflict);
        }
        self.allocations.admit_cleanup_job(&write, &permit).await
    }
}
