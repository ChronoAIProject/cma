use crate::Caller;

use super::*;

impl ResourceAccessService {
    /// Compatibility boundaries remain personal-only, but still read the actual
    /// owner kind. A retained organization sidecar cannot become a person by
    /// passing its raw owner ID through an older interface.
    pub async fn authorize_personal(
        &self,
        caller: &Caller,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<AuthorizedResource, ResourceAccessError> {
        self.require_account_available(&caller.subject, false)
            .await?;
        let snapshot = self.store.resource_access_snapshot(target).await?;
        let ResourceOwner::Person { subject } = snapshot.scope(target)?.owner else {
            return Err(AccessError::NotFound.into());
        };
        let decision = AccessDecision::personal(caller, &subject);
        decision.require(capability)?;
        self.require_owner_available(&snapshot.scope(target)?.owner, false)
            .await?;
        Ok(AuthorizedResource {
            snapshot,
            decision,
            binding: None,
        })
    }

    /// Fence a buffered result immediately before release. This does not renew
    /// membership evidence; expired decisions require a fresh authorization.
    pub async fn validate_read_result(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<(), ResourceAccessError> {
        self.current_read_snapshot(authorized, target, capability)
            .await
            .map(|_| ())
    }

    /// Obtain the next aggregate under the original policy and lifetime. This
    /// cannot renew a stream or adopt changed ownership/shares/target ancestry.
    pub(crate) async fn current_read_snapshot(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        authorized.decision.require(capability)?;
        let current = self.store.resource_access_snapshot(target).await?;
        self.validate_current_read_snapshot(authorized, target, capability, &current)
            .await?;
        Ok(current)
    }

    /// Validate a snapshot already read under the store's current transaction.
    /// Inventory release validation uses this to check every row while keeping
    /// all current reads on one Mongo session.
    pub(crate) async fn validate_current_read_snapshot(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
        current: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        authorized.decision.require(capability)?;
        let original_scope = authorized.snapshot.scope(target)?;
        self.require_owner_available(&original_scope.owner, false)
            .await?;
        if current.fence() != authorized.snapshot.fence()
            || current.scope(target)? != original_scope
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        self.validate_binding_authority(authorized).await?;
        authorized.decision.require(capability)?;
        Ok(())
    }
}
