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
        let snapshot = self.store.resource_access_snapshot(target).await?;
        let ResourceOwner::Person { subject } = snapshot.scope(target)?.owner else {
            return Err(AccessError::NotFound.into());
        };
        let decision = AccessDecision::personal(caller, &subject);
        decision.require(capability)?;
        Ok(AuthorizedResource { snapshot, decision })
    }

    /// Fence a buffered result immediately before release. This does not renew
    /// membership evidence; expired decisions require a fresh authorization.
    pub async fn validate_read_result(
        &self,
        authorized: &AuthorizedResource,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<(), ResourceAccessError> {
        authorized.decision.require(capability)?;
        let original_scope = authorized.snapshot.scope(target)?;
        let current = self.store.resource_access_snapshot(target).await?;
        if current.fence() != authorized.snapshot.fence()
            || current.scope(target)? != original_scope
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        authorized.decision.require(capability)?;
        Ok(())
    }
}
