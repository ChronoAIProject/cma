use super::*;
use crate::{
    bridge::BridgeId,
    resource_access::{
        AccessPurpose, CallerContext, ResourceActor, ResourceCapability, ResourceTarget,
    },
};
use std::time::Duration;

impl BridgeCleanupService {
    pub(super) async fn management(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
    ) -> Result<BridgeManagementLease, CleanupError> {
        if !context.validate_principal()
            || !matches!(
                context.actor,
                ResourceActor::Human { .. }
                    | ResourceActor::Api {
                        delegation_id: None,
                        ..
                    }
            )
        {
            return Err(CleanupError::NotFound);
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let record = self
            .definitions
            .store
            .bridge(bridge)
            .await?
            .ok_or(CleanupError::NotFound)?;
        let mut management = self
            .definitions
            .management_lease(context, credential, bridge, record.revision)
            .await?;
        management.bound_deadline(deadline);
        management.require_current()?;
        Ok(management)
    }

    pub(super) async fn access(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
        purpose: AccessPurpose,
    ) -> Result<CleanupAccess, CleanupError> {
        if !allocation_id(id) {
            return Err(CleanupError::Invalid);
        }
        let mut management = self.management(context, credential, bridge).await?;
        let allocation = self
            .store
            .allocation(&management, id)
            .await?
            .ok_or(CleanupError::NotFound)?;
        allocation.validate().map_err(|_| CleanupError::Corrupt)?;
        if allocation.id != id
            || allocation.identity.bridge_id != *bridge
            || allocation.owner != management.record().owner
        {
            return Err(CleanupError::NotFound);
        }
        let parent = self
            .resources
            .authorize(
                context,
                credential,
                &ResourceTarget::Sandbox {
                    sandbox_id: allocation.placement.target().sandbox_id.clone(),
                },
                ResourceCapability::ManageSandbox,
                purpose,
            )
            .await
            .map_err(CleanupError::from)?;
        if let Some(deadline) = parent
            .decision
            .revalidation_deadline(ResourceCapability::ManageSandbox)
        {
            management.bound_deadline(deadline);
        }
        let access = CleanupAccess {
            management,
            parent,
            allocation,
        };
        access.require_current()?;
        Ok(access)
    }

    pub(super) async fn revalidate(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        access: &CleanupAccess,
    ) -> Result<(), CleanupError> {
        access.require_current()?;
        self.definitions
            .validate_management(&access.management)
            .await?;
        let current = self
            .resources
            .authorize(
                context,
                credential,
                &ResourceTarget::Sandbox {
                    sandbox_id: access.allocation.placement.target().sandbox_id.clone(),
                },
                ResourceCapability::ManageSandbox,
                AccessPurpose::Read,
            )
            .await
            .map_err(CleanupError::from)?;
        if current.snapshot != access.parent.snapshot {
            return Err(CleanupError::Conflict);
        }
        access.require_current()
    }
}
