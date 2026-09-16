use super::*;
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};

pub(super) struct TargetAccess {
    pub redacted: bool,
    pub checked: Vec<(ResourceTarget, ResourceCapability, AuthorizedResource)>,
}

impl BridgePartitionService {
    pub(super) async fn authorize_targets(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        manager: &mut BridgeManagementLease,
        evidence: &PartitionEvidence,
    ) -> Result<TargetAccess, PartitionStatusError> {
        let mut result = TargetAccess {
            redacted: false,
            checked: Vec::new(),
        };
        let ids = evidence.allocation.placement.target();
        if evidence.present.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(PartitionStatusError::Corrupt);
        }
        for kind in &evidence.present {
            manager.require_current()?;
            let target = kind.target(ids);
            let capability = if *kind == PartitionResource::Agent {
                ResourceCapability::ViewHistory
            } else {
                ResourceCapability::ViewDetails
            };
            let access = match self
                .resources
                .authorize(
                    context,
                    credential,
                    &target,
                    capability,
                    AccessPurpose::Read,
                )
                .await
            {
                Ok(access) => access,
                Err(error) if denied(&error) => {
                    result.redacted = true;
                    continue;
                }
                Err(error) => return Err(access_error(error)),
            };
            access
                .decision
                .require(capability)
                .map_err(|_| PartitionStatusError::Unavailable)?;
            let lineage = &access.snapshot.lineage;
            if access
                .snapshot
                .scope(&target)
                .map_err(|_| PartitionStatusError::Corrupt)?
                .owner
                != evidence.allocation.owner
                || lineage.sandbox.id != ids.sandbox_id
                || lineage
                    .workspace
                    .as_ref()
                    .is_some_and(|v| v.id != ids.workspace_id)
                || lineage
                    .worktree
                    .as_ref()
                    .is_some_and(|v| v.id != ids.worktree_id)
                || lineage.agent.as_ref().is_some_and(|v| v.id != ids.agent_id)
            {
                return Err(PartitionStatusError::Corrupt);
            }
            if let Some(deadline) = access.decision.revalidation_deadline(capability) {
                manager.bound_deadline(deadline);
            }
            result.checked.push((target, capability, access));
        }
        manager.require_current()?;
        Ok(result)
    }

    pub(super) async fn revalidate_targets(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        manager: &BridgeManagementLease,
        access: &TargetAccess,
    ) -> Result<(), PartitionStatusError> {
        for (target, capability, original) in &access.checked {
            manager.require_current()?;
            original
                .decision
                .require(*capability)
                .map_err(|_| PartitionStatusError::Unavailable)?;
            let current = self
                .resources
                .authorize(
                    context,
                    credential,
                    target,
                    *capability,
                    AccessPurpose::Read,
                )
                .await
                .map_err(access_error)?;
            current
                .decision
                .require(*capability)
                .map_err(|_| PartitionStatusError::Unavailable)?;
            if current.snapshot != original.snapshot {
                return Err(PartitionStatusError::Conflict);
            }
        }
        manager.require_current()?;
        Ok(())
    }
}

fn denied(error: &ResourceAccessError) -> bool {
    matches!(
        error,
        ResourceAccessError::Access(
            AccessError::NotFound | AccessError::Forbidden | AccessError::UnsafeInteractiveShare
        ) | ResourceAccessError::Store(
            ResourceAccessStoreError::NotFound | ResourceAccessStoreError::UnsafeInteractiveShare
        )
    )
}
fn access_error(error: ResourceAccessError) -> PartitionStatusError {
    if denied(&error) {
        return PartitionStatusError::Conflict;
    }
    match error {
        ResourceAccessError::Store(
            ResourceAccessStoreError::Corrupt | ResourceAccessStoreError::Invalid,
        ) => PartitionStatusError::Corrupt,
        ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict | ResourceAccessStoreError::IdempotencyConflict,
        ) => PartitionStatusError::Conflict,
        _ => PartitionStatusError::Unavailable,
    }
}
