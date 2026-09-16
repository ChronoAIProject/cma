//! Effect-free live preflight, followed by a fenced durable reservation.
use super::{provisioning::BridgeProvisioningAuthority, ManagedBridgeCredentials};
use crate::store::MongoBridgeAllocationStore;
use layer2_core::bridge::{
    allocation::{AllocationAuthority, AllocationPlacement},
    invoke::{BridgeInvokeEffect, BridgeInvokeGrantId, BridgeInvokeRequest, BridgeInvokeService},
    partition_commands::*,
    BridgeFeature, BridgeManagementLease, BridgeService,
};
use std::sync::Arc;

pub(crate) struct ManagedPartitionCommands {
    definitions: BridgeService,
    allocations: Arc<MongoBridgeAllocationStore>,
    invokes: BridgeInvokeService,
    authority: Option<Arc<BridgeProvisioningAuthority>>,
}
impl ManagedPartitionCommands {
    pub(crate) fn new(
        definitions: BridgeService,
        allocations: Arc<MongoBridgeAllocationStore>,
        invokes: BridgeInvokeService,
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
            invokes,
            authority,
        }
    }
}
#[async_trait::async_trait]
impl BridgePartitionCommandStore for ManagedPartitionCommands {
    async fn recover(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
        digest: &str,
    ) -> Result<Option<PartitionCommandReceipt>, PartitionCommandError> {
        self.allocations
            .recover_partition_command(manager, id, digest)
            .await
    }
    async fn snapshot(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<PartitionCommandSnapshot, PartitionCommandError> {
        self.allocations
            .partition_command_snapshot(manager, id)
            .await
    }
    async fn apply(
        &self,
        write: PartitionCommandWrite,
    ) -> Result<PartitionCommandReceipt, PartitionCommandError> {
        if let Some(receipt) = self
            .recover(
                write.management(),
                write.receipt_id(),
                write.request_sha256(),
            )
            .await?
        {
            return Ok(receipt);
        }
        write.require_current()?;
        let reset = if write.request().action == PartitionCommandAction::Reset {
            let previous = &write.snapshot().allocation;
            self.definitions
                .check_feature_for_pin(BridgeFeature::Allocation, &previous.pin)
                .await?;
            let admission = write.admission()?;
            let authority = match &previous.placement {
                AllocationPlacement::ExistingAgent { target, grant_id } => {
                    let proof = self
                        .invokes
                        .authorize(BridgeInvokeRequest {
                            grant_id: BridgeInvokeGrantId(grant_id.clone()),
                            pin: previous.pin.clone(),
                            target: target.clone(),
                            effect: BridgeInvokeEffect::BindExistingTarget {
                                allocation_id: admission.identity().id()?,
                            },
                        })
                        .await
                        .map_err(|_| PartitionCommandError::Unavailable)?;
                    AllocationAuthority::Existing(Box::new(proof))
                }
                AllocationPlacement::NewSandbox { .. }
                | AllocationPlacement::ReuseParent { .. } => {
                    self.authority
                        .as_ref()
                        .ok_or(PartitionCommandError::Unavailable)?
                        .prepare_accepted(&self.definitions, admission.identity(), &previous.pin)
                        .await?
                        .0
                }
            };
            Some(write.reset(authority, mongodb::bson::DateTime::now().timestamp_millis())?)
        } else {
            None
        };
        self.definitions
            .validate_management(write.management())
            .await?;
        self.allocations
            .apply_partition_command(&write, reset.as_ref())
            .await
    }
}
