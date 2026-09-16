use super::*;
use crate::{
    bridge::{
        allocation::{AllocationPlacement, AllocationState},
        BridgeFeature, BridgeId, BridgeService,
    },
    resource_access::{AccessPurpose, CallerContext, ResourceAccessService},
};
use std::{sync::Arc, time::SystemTime};

#[derive(Clone)]
pub struct BridgeCleanupService {
    pub(super) definitions: BridgeService,
    pub(super) resources: Arc<ResourceAccessService>,
    pub(super) store: Arc<dyn BridgeCleanupStore>,
}

impl BridgeCleanupService {
    pub fn new(
        definitions: BridgeService,
        resources: Arc<ResourceAccessService>,
        store: Arc<dyn BridgeCleanupStore>,
    ) -> Self {
        Self {
            definitions,
            resources,
            store,
        }
    }

    pub async fn status(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        id: &str,
    ) -> Result<ChildCleanupStatus, CleanupError> {
        let access = self
            .access(context, credential, bridge, id, AccessPurpose::Read)
            .await?;
        let status = self.store.status(&access).await?;
        access.require_status(&status)?;
        self.revalidate(context, credential, &access).await?;
        Ok(status)
    }

    pub async fn submit(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        command: ChildCleanupCommand,
    ) -> Result<ChildCleanupStatus, CleanupError> {
        let ChildCleanupCommand {
            allocation_id: allocation,
            idempotency_key: key,
            request,
        } = command;
        request.validate()?;
        if key.is_empty() || key.len() > 200 || !key.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(CleanupError::Invalid);
        }
        let access = self
            .access(
                context,
                credential,
                bridge,
                &allocation,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?;
        let receipt_id = format!(
            "bclj_{}",
            digest(&(
                "cma.bridge.child-cleanup.v1",
                &context.credential.issuer,
                &context.actor,
                bridge,
                &allocation,
                &key
            ))?
        );
        let request_sha256 = digest(&request)?;
        if let Some(status) = self
            .store
            .recover(&access, &receipt_id, &request_sha256)
            .await?
        {
            access.require_status(&status)?;
            let receipt = status.receipt.as_ref().ok_or(CleanupError::Corrupt)?;
            if receipt.id != receipt_id
                || receipt.request != request
                || receipt.requested_by != context.actor
            {
                return Err(CleanupError::Corrupt);
            }
            self.revalidate(context, credential, &access).await?;
            return Ok(status);
        }
        self.definitions.check_feature(
            BridgeFeature::Management,
            &access.allocation.config.connector.platform,
        )?;
        if access.allocation.state != AllocationState::Retired
            || !matches!(
                access.allocation.placement,
                AllocationPlacement::ReuseParent { .. }
            )
        {
            return Err(CleanupError::Conflict);
        }
        let write = CleanupWrite {
            access,
            request,
            receipt_id,
            request_sha256,
            issuer: context.credential.issuer.clone(),
        };
        write.require_current()?;
        // The store repeats original-receipt recovery and commits the explicit
        // job with its request/audit and exact current authority fences.
        let status = self.store.submit(write).await?;
        // Re-establish current read authority after an accepted transaction;
        // a lost/revoked read does not undo its durable keyed acceptance.
        let access = self
            .access(
                context,
                credential,
                bridge,
                &allocation,
                AccessPurpose::Read,
            )
            .await?;
        access.require_status(&status)?;
        self.revalidate(context, credential, &access).await?;
        Ok(status)
    }
}
