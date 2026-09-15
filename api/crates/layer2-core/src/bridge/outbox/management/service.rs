use super::*;
use crate::{
    bridge::{BridgeError, BridgeId, BridgeManagementLease, BridgeService},
    resource_access::CallerContext,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct BridgeOutboxService {
    definitions: BridgeService,
    store: Arc<dyn BridgeOutboxManagementStore>,
}
impl BridgeOutboxService {
    pub fn new(definitions: BridgeService, store: Arc<dyn BridgeOutboxManagementStore>) -> Self {
        Self { definitions, store }
    }
    async fn management(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
    ) -> Result<BridgeManagementLease, OutboxManagementError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let bridge = self
            .definitions
            .store
            .bridge(bridge_id)
            .await?
            .ok_or(BridgeError::NotFound)?;
        let mut management = self
            .definitions
            .management_lease(context, credential, bridge_id, bridge.revision)
            .await?;
        management.bound_deadline(deadline);
        management.require_current()?;
        Ok(management)
    }
    pub async fn list(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
        after: Option<&str>,
        limit: u32,
    ) -> Result<OutboxPage, OutboxManagementError> {
        if !(1..=100).contains(&limit) || after.is_some_and(|id| !item_id(id)) {
            return Err(OutboxManagementError::Invalid);
        }
        let manager = self.management(context, credential, bridge_id).await?;
        let mut items = self.store.list(&manager, after, limit).await?;
        if items.len() > limit as usize + 1 {
            return Err(OutboxError::Corrupt.into());
        }
        let mut previous = after;
        for item in &items {
            require_source(&manager, item)?;
            if previous.is_some_and(|id| id >= item.id.as_str()) {
                return Err(OutboxError::Corrupt.into());
            }
            previous = Some(&item.id);
        }
        let more = items.len() > limit as usize;
        items.truncate(limit as usize);
        let next_cursor = if more {
            items.last().map(|item| item.id.clone())
        } else {
            None
        };
        self.definitions.validate_management(&manager).await?;
        Ok(OutboxPage {
            items: items.iter().map(OutboxItemView::from).collect(),
            next_cursor,
        })
    }
    pub async fn detail(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
        id: &str,
    ) -> Result<OutboxItemView, OutboxManagementError> {
        if !item_id(id) {
            return Err(OutboxManagementError::Invalid);
        }
        let manager = self.management(context, credential, bridge_id).await?;
        let item = self
            .store
            .item(&manager, id)
            .await?
            .ok_or(OutboxManagementError::NotFound)?;
        require_source(&manager, &item)?;
        self.definitions.validate_management(&manager).await?;
        Ok((&item).into())
    }
    pub async fn command(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge_id: &BridgeId,
        key: &str,
        request: OutboxRecoveryRequest,
    ) -> Result<OutboxRecoveryReceipt, OutboxManagementError> {
        if !super::super::id(key)
            || !item_id(&request.item_id)
            || request.expected_revision == 0
            || request.expected_bridge_revision == 0
            || request.expected_revision > i64::MAX as u64
            || request.expected_bridge_revision > i64::MAX as u64
        {
            return Err(OutboxManagementError::Invalid);
        }
        let manager = self.management(context, credential, bridge_id).await?;
        let receipt_id = format!(
            "brrc_{}",
            super::super::hash(&(
                "cma.bridge.outbox-recovery.v1",
                &context.credential.issuer,
                manager.actor(),
                bridge_id,
                key
            ))?
        );
        let request_sha256 = super::super::hash(&request)?;
        // Recover an already committed command before current item CAS or
        // provider availability: neither can justify repeating the mutation.
        if let Some(receipt) = self
            .store
            .recover(&manager, &receipt_id, &request_sha256)
            .await?
        {
            return Ok(receipt);
        }
        let item = self
            .store
            .item(&manager, &request.item_id)
            .await?
            .ok_or(OutboxManagementError::NotFound)?;
        require_source(&manager, &item)?;
        if item.revision != request.expected_revision
            || manager.record().revision != request.expected_bridge_revision
        {
            return Err(OutboxError::Conflict.into());
        }
        if matches!(item.state, super::super::DeliveryState::Unknown { .. })
            && matches!(
                request.action,
                OutboxRecoveryAction::Retry {
                    accept_duplicate_risk: false
                }
            )
        {
            return Err(OutboxManagementError::DuplicateRiskRequired);
        }
        let validation = if matches!(request.action, OutboxRecoveryAction::Retry { .. }) {
            self.definitions.check_feature(
                crate::bridge::BridgeFeature::Management,
                &item.source.config().connector.platform,
            )?;
            Some(
                self.definitions
                    .connection(item.source.owner(), item.source.config())
                    .await?,
            )
        } else {
            None
        };
        let write = OutboxRecoveryWrite {
            management: manager,
            request,
            receipt_id,
            request_sha256,
            validation,
        };
        write.require_current()?;
        self.store.apply(write).await
    }
}
fn item_id(id: &str) -> bool {
    id.strip_prefix("bro_").is_some_and(super::super::digest)
}
