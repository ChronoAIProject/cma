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
        if !matches!(
            record.state,
            AllocationState::Ready | AllocationState::Retired | AllocationState::FailedTerminal
        ) {
            super::operations::health::pending(
                super::operations::health::PendingStage::Allocation,
                &record.id,
                record.created_at_ms,
                false,
            );
        }
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
            .map_err(|error| bridge_failure("allocation_feature", error))?;
        let (authority, lease) = self
            .authority
            .resolve(&self.definitions, &record)
            .await
            .map_err(|error| bridge_failure("allocation_authority", error))?;
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
            ChannelProvisioningOutcome::MintOutcomeUnknown(record) => {
                super::operations::health::mint_unknown(&record.id);
                if record.id != id {
                    return Err(Error::Corrupt);
                }
                Ok(())
            }
            ChannelProvisioningOutcome::Progressed(record) => {
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
        let state = match result {
            Ok(()) => "waiting_allocation",
            Err(Error::Changed) => "changed",
            Err(Error::Unsupported) => "unsupported",
            Err(Error::Suspended) => "authority_required",
            Err(Error::Corrupt) => "inconsistent",
            Err(Error::AttachmentExpired) => "text_attachment_expired",
            Err(Error::AttachmentUnavailable) => "text_attachment_unavailable",
            Err(Error::Unavailable) => "unavailable",
        };
        let delay = result
            .err()
            .map_or(Duration::from_secs(1), Error::retry_delay);
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
        super::operations::health::pending(
            super::operations::health::PendingStage::Inbox,
            &inbox.id,
            inbox.accepted_at.saturating_mul(1000),
            inbox.execution_hold.is_some(),
        );
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
                    .map_err(|error| bridge_failure("allocation_feature", error))?;
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
                            .map_err(|error| bridge_failure("prepare_allocation", error))?
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
                            .map_err(|error| invoke_failure("bind_existing_target", error))?;
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
            .map_err(|error| bridge_failure("execution_feature", error))?;
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
                match self.retained_channel_grant(inbox, &record).await? {
                    Some(grant) => grant,
                    None => {
                        let (authority, lease) = self
                            .authority
                            .resolve(&self.definitions, &record)
                            .await
                            .map_err(|error| bridge_failure("channel_authority", error))?;
                        self.invokes
                            .issue_channel(Arc::new(lease), &record.id, &inbox.pin, authority)
                            .await
                            .map_err(|error| invoke_failure("issue_channel_grant", error))?
                            .id
                    }
                }
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
            .map_err(|error| invoke_failure("authorize_channel_input", error))?;
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

    async fn retained_channel_grant(
        &self,
        inbox: &StoredInbox,
        allocation: &AllocationRecord,
    ) -> Result<Option<BridgeInvokeGrantId>, Error> {
        let mut after = None;
        loop {
            let rows = self
                .invoke_store
                .list(&inbox.pin.bridge_id, after.as_ref(), 100)
                .await
                .map_err(|error| invoke_failure("retained_channel_grant", error))?;
            if rows.len() > 101 {
                return Err(Error::Corrupt);
            }
            for grant in rows.iter().take(100) {
                grant.validate().map_err(|_| Error::Corrupt)?;
                if matches_retained_channel_grant(grant, inbox, allocation) {
                    // A retained grant selects identity only. Fresh authorize
                    // below and the admission transaction still check current
                    // custody, revocation, resource ownership and provenance.
                    // Return revoked matches too: never remint to bypass denial.
                    return Ok(Some(grant.id.clone()));
                }
            }
            if rows.len() <= 100 {
                return Ok(None);
            }
            let next = rows[99].id.clone();
            if after.as_ref().is_some_and(|prior| prior >= &next) {
                return Err(Error::Corrupt);
            }
            after = Some(next);
        }
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

pub(crate) fn matches_retained_channel_grant(
    grant: &layer2_core::bridge::invoke::BridgeInvokeGrant,
    inbox: &StoredInbox,
    allocation: &AllocationRecord,
) -> bool {
    grant.pin == inbox.pin
        && grant.owner == inbox.owner
        && grant.target.ids == *allocation.placement.target()
        && grant.channel.as_ref().is_some_and(|basis| {
            basis.allocation.id == allocation.id
                && basis.allocation.identity == allocation.identity
                && basis.allocation.pin == allocation.pin
        })
}

fn bridge_failure(stage: &'static str, failure: layer2_core::bridge::BridgeError) -> Error {
    use layer2_core::bridge::BridgeError;
    let classified = match failure {
        BridgeError::Unavailable | BridgeError::AuthorityUnavailable => Error::Unavailable,
        BridgeError::Conflict => Error::Changed,
        BridgeError::Corrupt => Error::Corrupt,
        _ => Error::Suspended,
    };
    tracing::info!(target: "cma::bridge::operations", operation="admission_failure", stage,
        failure=%failure, classification=?classified, "channel_admission_failure");
    classified
}

fn invoke_failure(
    stage: &'static str,
    failure: layer2_core::bridge::invoke::BridgeInvokeError,
) -> Error {
    use layer2_core::bridge::invoke::BridgeInvokeError;
    if let BridgeInvokeError::Bridge(error) = failure {
        return bridge_failure(stage, error);
    }
    let classified = match failure {
        BridgeInvokeError::Unavailable => Error::Unavailable,
        BridgeInvokeError::Conflict => Error::Changed,
        BridgeInvokeError::Corrupt => Error::Corrupt,
        _ => Error::Suspended,
    };
    tracing::info!(target: "cma::bridge::operations", operation="admission_failure", stage,
        failure=%failure, classification=?classified, "channel_admission_failure");
    classified
}

#[cfg(test)]
#[path = "channel_work_tests.rs"]
mod tests;
