use std::{sync::Arc, time::Duration};

use layer2_core::bridge::{
    allocation::{
        AllocationAdmission, AllocationAuthority, AllocationPlacement, AllocationRecord,
        AllocationReservation, AllocationState, AllocationStore,
    },
    ingress::StoredInbox,
    invoke::{
        BridgeInvokeEffect, BridgeInvokeGrantId, BridgeInvokeGrantState, BridgeInvokeRequest,
        BridgeInvokeService, BridgeInvokeStore,
    },
    partition::NormalizedPartitionSource,
    BridgePlacementPolicy, BridgeService,
};
use layer2_core::sandbox::agent_io::ChannelAgentCommandService;

use super::{
    channel_scheduler::{ChannelWorkError as Error, ChannelWorkPort},
    provisioning::BridgeProvisioningAuthority,
    ManagedBridgeCredentials,
};
use crate::{
    sandbox::{
        channel_children::ChannelChildDriver,
        channel_provisioning::{ChannelProvisioningDriver, ChannelProvisioningOutcome},
        channel_startup::ChannelStartupDriver,
    },
    store::{MongoBridgeAllocationStore, MongoBridgeInvokeStore, MongoCallbackInbox},
};

pub(crate) struct ChannelAllocationWork {
    pub definitions: BridgeService,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<BridgeProvisioningAuthority>,
    pub driver: ChannelProvisioningDriver,
    pub children: ChannelChildDriver,
    pub startup: ChannelStartupDriver,
    pub reuse: crate::sandbox::channel_reuse::ChannelReuseDriver,
}

#[async_trait::async_trait]
impl ChannelWorkPort for ChannelAllocationWork {
    fn kind(&self) -> super::operations::WorkKind {
        super::operations::WorkKind::Allocation
    }
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, Error> {
        self.allocations
            .pending_channel_allocations(after)
            .await
            .map_err(|_| Error::Unavailable)
    }
    async fn run_one(&self, id: &str) -> Result<(), Error> {
        let Some(record) = self
            .allocations
            .channel_allocation(id)
            .await
            .map_err(|_| Error::Corrupt)?
        else {
            return Ok(());
        };
        if record
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at_ms > now_ms())
        {
            return Ok(());
        }
        self.definitions
            .check_feature_for_pin(layer2_core::bridge::BridgeFeature::Allocation, &record.pin)
            .await
            .map_err(|_| Error::Suspended)?;
        let (authority, lease) = self
            .authority
            .resolve(&self.definitions, &record)
            .await
            .map_err(|_| Error::Suspended)?;
        if matches!(record.placement, AllocationPlacement::ReuseParent { .. }) {
            self.reuse
                .run_one(record, &authority, &lease)
                .await
                .map_err(|_| Error::Unavailable)?;
            return Ok(());
        }
        match self
            .driver
            .run_one(record, &authority, &lease)
            .await
            .map_err(|_| Error::Unavailable)?
        {
            ChannelProvisioningOutcome::Progressed(record)
            | ChannelProvisioningOutcome::MintOutcomeUnknown(record) => {
                // Consuming the returned identity does not reissue its effects.
                if record.id != id {
                    return Err(Error::Corrupt);
                }
                Ok(())
            }
            ChannelProvisioningOutcome::Children(snapshot) => {
                if snapshot.state.phase == crate::sandbox::channel_provisioning::state::ChannelProvisioningPhase::AgentStart {
                    self.startup.advance(&authority, &lease, &snapshot).await
                        .map_err(|_| Error::Unavailable)?;
                } else {
                    self.children.advance(&authority, &lease, &snapshot).await
                        .map_err(|_| Error::Unavailable)?;
                }
                Ok(())
            }
        }
    }
}

pub(crate) struct ChannelInboxWork {
    pub definitions: BridgeService,
    pub credentials: Arc<ManagedBridgeCredentials>,
    pub inbox: Arc<MongoCallbackInbox>,
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub authority: Arc<BridgeProvisioningAuthority>,
    pub invokes: BridgeInvokeService,
    pub invoke_store: Arc<MongoBridgeInvokeStore>,
    pub commands: Arc<ChannelAgentCommandService>,
}

#[async_trait::async_trait]
impl ChannelWorkPort for ChannelInboxWork {
    fn kind(&self) -> super::operations::WorkKind {
        super::operations::WorkKind::Inbox
    }
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, Error> {
        self.inbox
            .pending_channel_inputs(after)
            .await
            .map_err(|_| Error::Unavailable)
    }
    async fn run_one(&self, id: &str) -> Result<(), Error> {
        let result = self.advance(id).await;
        let (delay, state) = match result {
            Ok(()) => (Duration::from_secs(1), "waiting_allocation"),
            Err(Error::Changed) => (Duration::from_secs(1), "changed"),
            Err(Error::Unsupported) => (Duration::from_secs(30), "unsupported"),
            Err(Error::Suspended) => (Duration::from_secs(30), "authority_required"),
            Err(Error::Corrupt) => (Duration::from_secs(30), "inconsistent"),
            Err(Error::AttachmentExpired) => (Duration::from_secs(30), "text_attachment_expired"),
            Err(Error::AttachmentUnavailable) => {
                (Duration::from_secs(5), "text_attachment_unavailable")
            }
            Err(Error::Unavailable) => (Duration::from_secs(5), "unavailable"),
        };
        self.inbox
            .defer_channel_input(id, delay, state)
            .await
            .map_err(|_| Error::Unavailable)?;
        result
    }
}

impl ChannelInboxWork {
    async fn advance(&self, id: &str) -> Result<(), Error> {
        let Some(inbox) = self
            .inbox
            .channel_input(id)
            .await
            .map_err(|_| Error::Corrupt)?
        else {
            return Ok(());
        };
        let bridge = self
            .credentials
            .bridges
            .bridge(&inbox.pin.bridge_id)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::Changed)?;
        let source =
            NormalizedPartitionSource::from_stored(&inbox, &bridge).map_err(|_| Error::Corrupt)?;
        let admission =
            AllocationAdmission::from_accepted(bridge, &source).map_err(|_| Error::Suspended)?;
        let current = self
            .allocations
            .current(admission.identity())
            .await
            .map_err(|_| Error::Unavailable)?;
        let record = match current {
            Some(record) => record,
            None => {
                self.definitions
                    .check_feature_for_pin(
                        layer2_core::bridge::BridgeFeature::Allocation,
                        &inbox.pin,
                    )
                    .await
                    .map_err(|_| Error::Suspended)?;
                let config = self
                    .credentials
                    .bridges
                    .bridge(&inbox.pin.bridge_id)
                    .await
                    .map_err(|_| Error::Unavailable)?
                    .ok_or(Error::Changed)?
                    .pinned(&inbox.pin)
                    .map_err(|_| Error::Changed)?
                    .config
                    .clone();
                let authority = match &config.placement {
                    BridgePlacementPolicy::NewSandbox { .. }
                    | BridgePlacementPolicy::WorkspacePerPartition { .. }
                    | BridgePlacementPolicy::WorktreeAgentPerPartition { .. } => {
                        self.authority
                            .prepare_accepted(&self.definitions, admission.identity(), &inbox.pin)
                            .await
                            .map_err(|_| Error::Suspended)?
                            .0
                    }
                    BridgePlacementPolicy::ExistingAgent { target } => {
                        let grant = self.existing_grant(&inbox, target).await?;
                        let proof = self
                            .invokes
                            .authorize(BridgeInvokeRequest {
                                grant_id: grant,
                                pin: inbox.pin.clone(),
                                target: target.clone(),
                                effect: BridgeInvokeEffect::BindExistingTarget {
                                    allocation_id: admission
                                        .identity()
                                        .id()
                                        .map_err(|_| Error::Corrupt)?,
                                },
                            })
                            .await
                            .map_err(|_| Error::Suspended)?;
                        AllocationAuthority::Existing(Box::new(proof))
                    }
                };
                self.allocations
                    .reserve(
                        AllocationReservation::new(admission, authority, now_ms())
                            .map_err(|_| Error::Changed)?,
                    )
                    .await
                    .map_err(|_| Error::Unavailable)?
            }
        };
        self.admit(&inbox, record).await
    }

    async fn admit(&self, inbox: &StoredInbox, record: AllocationRecord) -> Result<(), Error> {
        if record.state != AllocationState::Ready {
            return Ok(());
        }
        self.definitions
            .check_feature_for_pin(layer2_core::bridge::BridgeFeature::Execution, &inbox.pin)
            .await
            .map_err(|_| Error::Suspended)?;
        if record.pin != inbox.pin
            && matches!(record.placement, AllocationPlacement::ExistingAgent { .. })
        {
            return Err(Error::Changed);
        }
        let grant_id = match &record.placement {
            AllocationPlacement::ExistingAgent { grant_id, .. } => {
                BridgeInvokeGrantId(grant_id.clone())
            }
            AllocationPlacement::NewSandbox { .. } | AllocationPlacement::ReuseParent { .. } => {
                let (authority, lease) = self
                    .authority
                    .resolve(&self.definitions, &record)
                    .await
                    .map_err(|_| Error::Suspended)?;
                self.invokes
                    .issue_channel(Arc::new(lease), &record.id, &inbox.pin, authority)
                    .await
                    .map_err(|_| Error::Suspended)?
                    .id
            }
        };
        let proof = self
            .invokes
            .authorize(BridgeInvokeRequest {
                grant_id,
                pin: inbox.pin.clone(),
                target: record.placement.target().clone(),
                effect: BridgeInvokeEffect::AdmitMessage {
                    inbox_id: inbox.id.clone(),
                },
            })
            .await
            .map_err(|_| Error::Suspended)?;
        let inbox = super::attachments::capture(
            inbox,
            &proof,
            &self.invokes,
            &self.inbox,
            &self.invoke_store,
        )
        .await?;
        self.commands
            .admit(proof, inbox, record)
            .await
            .map_err(|error| match error {
                layer2_core::sandbox::agent_io::ChannelCommandAdmissionError::Input(_) => {
                    Error::Unsupported
                }
                _ => Error::Unavailable,
            })?;
        Ok(())
    }

    async fn existing_grant(
        &self,
        inbox: &StoredInbox,
        target: &layer2_core::bridge::BridgeAgentTarget,
    ) -> Result<BridgeInvokeGrantId, Error> {
        let mut after = None;
        loop {
            let rows = self
                .invoke_store
                .list(&inbox.pin.bridge_id, after.as_ref(), 100)
                .await
                .map_err(|_| Error::Unavailable)?;
            if rows.len() > 101 {
                return Err(Error::Corrupt);
            }
            for row in rows.iter().take(100) {
                row.validate().map_err(|_| Error::Corrupt)?;
                if row.pin == inbox.pin
                    && row.target.ids == *target
                    && row.state == BridgeInvokeGrantState::Active
                    && row.channel.is_none()
                {
                    // Selection is only a candidate; authorize supplies the proof.
                    return Ok(row.id.clone());
                }
            }
            if rows.len() <= 100 {
                return Err(Error::Suspended);
            }
            after = rows.get(99).map(|row| row.id.clone());
        }
    }
}

fn now_ms() -> i64 {
    mongodb::bson::DateTime::now().timestamp_millis()
}
