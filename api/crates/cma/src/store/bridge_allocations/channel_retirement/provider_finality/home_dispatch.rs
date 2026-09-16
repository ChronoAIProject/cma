use super::dispatch::Change;
use super::*;
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::channel_provisioning::state::{ChannelProvisioningPhase, ChannelProvisioningSnapshot},
    store::MongoBridgeAllocationStore,
};
use layer2_core::bridge::allocation::AllocationAuthority;

impl MongoBridgeAllocationStore {
    pub(crate) async fn begin_channel_home_dispatch(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        request: &layer3_adapter::HomeVolumeRequest,
    ) -> Result<Option<String>, AllocationError> {
        let operation_id = format!(
            "{}:home:{}",
            snapshot.allocation.operation_id,
            uuid::Uuid::new_v4()
        );
        let recorded = self
            .change_runtime_dispatch(
                authority,
                human,
                snapshot,
                Change::HomeBegin {
                    operation_id: &operation_id,
                    request,
                },
            )
            .await?;
        Ok(recorded.then_some(operation_id))
    }

    pub(crate) async fn complete_channel_home_dispatch(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        operation_id: &str,
        receipt: &layer3_adapter::HomeProvisionReceipt,
    ) -> Result<(), AllocationError> {
        if !self
            .change_runtime_dispatch(
                authority,
                human,
                snapshot,
                Change::HomeComplete {
                    operation_id,
                    receipt,
                },
            )
            .await?
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }

    pub(crate) async fn record_channel_home_backing(
        &self,
        authority: &AllocationAuthority,
        human: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        backing: &layer3_adapter::HomeBackingIdentity,
    ) -> Result<(), AllocationError> {
        if !self
            .change_runtime_dispatch(authority, human, snapshot, Change::HomeBacking { backing })
            .await?
        {
            return Err(AllocationError::Conflict);
        }
        Ok(())
    }
}

pub(super) fn apply(
    journal: &mut DispatchJournal,
    snapshot: &ChannelProvisioningSnapshot,
    change: &Change<'_>,
) -> Result<(), AllocationError> {
    if snapshot.sandbox.runtime.is_some() {
        return Err(AllocationError::InvalidTransition);
    }
    let policy = journal
        .origin
        .original_home_policy
        .as_ref()
        .ok_or(AllocationError::IdentityMismatch)?;
    match change {
        Change::HomeBegin {
            operation_id,
            request,
        } => {
            let original = snapshot
                .home_provenance
                .as_ref()
                .ok_or(AllocationError::IdentityMismatch)?;
            if snapshot.state.phase != ChannelProvisioningPhase::Home
                || !journal.runtime.is_empty()
                || original.first_receipt.is_some()
                || original.request != **request
                || &request.policy != policy
                || journal.home.len() >= 64
            {
                return Err(AllocationError::IdentityMismatch);
            }
            journal.home.push(HomeDispatch {
                operation_id: (*operation_id).to_owned(),
                request: (*request).clone(),
                receipt: None,
                backing: None,
            });
        }
        Change::HomeComplete {
            operation_id,
            receipt,
        } => {
            if snapshot.state.phase != ChannelProvisioningPhase::Home || !journal.runtime.is_empty()
            {
                return Err(AllocationError::InvalidTransition);
            }
            receipt
                .validate()
                .map_err(|_| AllocationError::IdentityMismatch)?;
            let row = journal
                .home
                .iter_mut()
                .find(|row| row.operation_id == *operation_id)
                .ok_or(AllocationError::IdentityMismatch)?;
            if row.receipt.is_some()
                || receipt.operation_id() != row.operation_id
                || receipt.home().request != row.request
            {
                return Err(AllocationError::IdentityMismatch);
            }
            row.receipt = Some((*receipt).clone());
        }
        Change::HomeBacking { backing } => {
            if snapshot.state.phase != ChannelProvisioningPhase::Runtime
                || journal.home.len() != 1
                || journal.runtime.len() != 1
                || journal.runtime[0].receipt.is_none()
            {
                return Err(AllocationError::AuthorityUnavailable);
            }
            backing
                .validate()
                .map_err(|_| AllocationError::IdentityMismatch)?;
            let row = &mut journal.home[0];
            let receipt = row
                .receipt
                .as_ref()
                .ok_or(AllocationError::AuthorityUnavailable)?;
            let attachment = snapshot
                .sandbox
                .lifecycle
                .home
                .as_ref()
                .and_then(|home| home.attachment())
                .ok_or(AllocationError::IdentityMismatch)?;
            let provenance = snapshot
                .home_provenance
                .as_ref()
                .ok_or(AllocationError::IdentityMismatch)?;
            if receipt.home() != &attachment
                || backing.reference.home != attachment
                || backing.reference.provider_scope != provenance.scope
                || row
                    .backing
                    .as_ref()
                    .is_some_and(|previous| previous != *backing)
            {
                return Err(AllocationError::IdentityMismatch);
            }
            row.backing = Some((*backing).clone());
        }
        _ => return Err(AllocationError::InvalidTransition),
    }
    Ok(())
}
