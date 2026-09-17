use std::{sync::Arc, time::Duration};

use crate::resource_access::{
    CallerContext, ResourceAccessError, ResourceAccessService, ResourceAccessStoreError,
    ResourceShareStore,
};

use super::super::{BridgeId, BridgeManagementLease, BridgeService};
use super::*;

#[derive(Clone)]
pub struct BridgeInvokeService {
    pub(super) definitions: BridgeService,
    pub(super) access: Arc<ResourceAccessService>,
    pub(super) resources: Arc<dyn ResourceShareStore>,
    pub(super) store: Arc<dyn BridgeInvokeStore>,
}

impl BridgeInvokeService {
    pub fn new(
        definitions: BridgeService,
        access: Arc<ResourceAccessService>,
        resources: Arc<dyn ResourceShareStore>,
        store: Arc<dyn BridgeInvokeStore>,
    ) -> Self {
        Self {
            definitions,
            access,
            resources,
            store,
        }
    }

    pub(super) async fn management(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
        deadline: tokio::time::Instant,
    ) -> Result<BridgeManagementLease, BridgeInvokeError> {
        let bridge = self
            .definitions
            .store
            .bridge(id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let mut lease = self
            .definitions
            .management_lease(context, credential, id, bridge.revision)
            .await?;
        lease.bound_deadline(deadline);
        lease.require_current()?;
        Ok(lease)
    }

    pub async fn detail(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeInvokeGrantId,
    ) -> Result<BridgeInvokeGrantView, BridgeInvokeError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        id.validate()?;
        let grant = self
            .store
            .grant(id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let lease = self
            .management(context, credential, &grant.pin.bridge_id, deadline)
            .await?;
        require_owner(&grant, lease.record())?;
        lease.require_current()?;
        Ok(grant.view())
    }

    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
        after: Option<&BridgeInvokeGrantId>,
        limit: u32,
    ) -> Result<BridgeInvokePage, BridgeInvokeError> {
        if !(1..=100).contains(&limit) {
            return Err(BridgeInvokeError::Invalid("limit"));
        }
        if let Some(after) = after {
            after.validate()?;
        }
        let lease = self
            .management(
                context,
                credential,
                bridge_id,
                tokio::time::Instant::now() + Duration::from_secs(15),
            )
            .await?;
        let mut grants = self.store.list(bridge_id, after, limit).await?;
        if grants.len() > limit as usize + 1 {
            return Err(BridgeInvokeError::Corrupt);
        }
        let mut previous = after;
        for grant in &grants {
            require_owner(grant, lease.record())?;
            if previous.is_some_and(|id| id >= &grant.id) {
                return Err(BridgeInvokeError::Corrupt);
            }
            previous = Some(&grant.id);
        }
        let more = grants.len() > limit as usize;
        grants.truncate(limit as usize);
        let next_cursor = more.then(|| grants.last().expect("positive limit").id.clone());
        self.definitions.validate_management(&lease).await?;
        Ok(BridgeInvokePage {
            items: grants.iter().map(BridgeInvokeGrant::view).collect(),
            next_cursor,
        })
    }

    /// Trusted worker entry point. There is deliberately no human credential or
    /// CallerContext. Fresh selected owner custody and an explicit current grant
    /// establish this bridge's narrow permission for the exact retained target.
    pub async fn authorize(
        &self,
        request: BridgeInvokeRequest,
    ) -> Result<BridgeInvokeProof, BridgeInvokeError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        request.grant_id.validate()?;
        request.effect.validate()?;
        let grant = self
            .store
            .grant(&request.grant_id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        grant.require_active()?;
        if grant.pin != request.pin || grant.target.ids != request.target {
            return Err(BridgeInvokeError::TargetMismatch);
        }
        let bridge = self
            .definitions
            .store
            .bridge(&grant.pin.bridge_id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        super::proof::require_bridge(&bridge, &grant, &request.effect)?;
        let resources = self
            .resources
            .resource_access_snapshot(&grant.target.resource())
            .await
            .map_err(resource_store_error)?;
        require_resource_owner(&grant, &resources)?;
        let connection = self
            .definitions
            .connection(&grant.owner, &grant.config)
            .await?;
        let mut proof = BridgeInvokeProof {
            deadline: deadline.min(connection.deadline()),
            grant,
            bridge,
            resources,
            connection,
            effect: request.effect,
        };
        let current_bridge = self
            .definitions
            .store
            .bridge(&proof.bridge.id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let current_grant = self
            .store
            .grant(&proof.grant.id)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let current_resources = self
            .resources
            .resource_access_snapshot(&proof.grant.target.resource())
            .await
            .map_err(resource_store_error)?;
        if matches!(
            proof.effect,
            BridgeInvokeEffect::AdmitMessage { .. } | BridgeInvokeEffect::ExecuteCommand { .. }
        ) {
            // Connection validation can overlap transcript or background-save
            // updates. Admission and execution fence the latest resource state;
            // those updates do not remove channel authority.
            // Keep the original policy and retained identity; the transaction
            // and ordered writer still check the exact turn and generation.
            if current_resources.fence() != proof.resources.fence()
                || current_resources.policy != proof.resources.policy
                || current_resources.shares != proof.resources.shares
            {
                return Err(BridgeInvokeError::TargetMismatch);
            }
            require_resource_owner(&proof.grant, &current_resources)?;
            proof.resources = current_resources.clone();
        }
        proof.require_snapshot(&current_bridge, &current_grant, &current_resources)?;
        Ok(proof)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeInvokePage {
    pub items: Vec<BridgeInvokeGrantView>,
    pub next_cursor: Option<BridgeInvokeGrantId>,
}

pub(super) fn require_owner(
    grant: &BridgeInvokeGrant,
    bridge: &super::super::BridgeRecord,
) -> Result<(), BridgeInvokeError> {
    grant.validate()?;
    if grant.owner != bridge.owner
        || grant.pin.bridge_id != bridge.id
        || grant.config.connector.issuer != bridge.current()?.config.connector.issuer
    {
        return Err(BridgeInvokeError::NotFound);
    }
    Ok(())
}

pub(super) fn require_resource_owner(
    grant: &BridgeInvokeGrant,
    resources: &crate::resource_access::ResourceAccessSnapshot,
) -> Result<(), BridgeInvokeError> {
    grant.target.require(&grant.owner, resources)?;
    if let Some(basis) = &grant.channel {
        basis.require_resources(resources)?;
    }
    if resources
        .policy
        .as_ref()
        .and_then(|policy| policy.organization_evidence.as_ref())
        .is_some_and(|evidence| evidence.issuer != grant.config.connector.issuer)
    {
        return Err(BridgeInvokeError::TargetMismatch);
    }
    Ok(())
}

pub(super) fn resource_store_error(error: ResourceAccessStoreError) -> BridgeInvokeError {
    match error {
        ResourceAccessStoreError::NotFound => BridgeInvokeError::NotFound,
        ResourceAccessStoreError::Conflict => BridgeInvokeError::Conflict,
        ResourceAccessStoreError::Unavailable => BridgeInvokeError::Unavailable,
        _ => BridgeInvokeError::TargetMismatch,
    }
}
pub(super) fn resource_error(error: ResourceAccessError) -> BridgeInvokeError {
    match error {
        ResourceAccessError::Access(crate::resource_access::AccessError::NotFound) => {
            BridgeInvokeError::NotFound
        }
        ResourceAccessError::Access(crate::resource_access::AccessError::Forbidden) => {
            BridgeInvokeError::Forbidden
        }
        ResourceAccessError::Store(error) => resource_store_error(error),
        _ => BridgeInvokeError::Unavailable,
    }
}
