use super::{
    home_prepare_evidence::ChannelHomePrepareEvidence, home_provenance::ChannelHomeProvenance,
    ChannelProvisioningDriver, ChannelProvisioningPhase, ChannelProvisioningSnapshot,
};
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError},
    sandbox::SandboxHome,
};

impl ChannelProvisioningDriver {
    pub(super) async fn home(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        let mut sandbox = snapshot.sandbox.clone();
        let mut provenance = snapshot.home_provenance.clone();
        let mut prepare_evidence = snapshot.home_prepare_evidence.clone();
        let (next, phase) = if let Some(original) = &snapshot.home_provenance {
            let homes = self
                .dependencies
                .adapter
                .home_volumes()
                .ok_or(AllocationError::AuthorityUnavailable)?;
            if homes.allocation_scope().as_ref() != Some(&original.scope) {
                return Err(AllocationError::IdentityMismatch);
            }
            let mut home = sandbox
                .lifecycle
                .home
                .as_deref()
                .cloned()
                .ok_or(AllocationError::Corrupt)?;
            if original.first_receipt.is_none() && prepare_evidence.is_none() {
                // A worker predating dispatch evidence may already have called
                // prepare. Record unresolved history before our next call; do
                // not infer that this is the first physical allocation attempt.
                prepare_evidence = Some(ChannelHomePrepareEvidence::capture(original)?);
                (None, ChannelProvisioningPhase::Home)
            } else if original.first_receipt.is_none() {
                // Even if an old worker has recorded a canonical receipt, only
                // this exact provider response may bind the separate provenance.
                let dispatch = self
                    .dependencies
                    .allocations
                    .begin_channel_home_dispatch(authority, lease, &snapshot, &original.request)
                    .await?;
                let receipt = if let Some(operation_id) = dispatch {
                    match homes
                        .prepare_receipted(&original.request, &operation_id)
                        .await
                    {
                        Ok(receipt) => {
                            self.dependencies
                                .allocations
                                .complete_channel_home_dispatch(
                                    authority,
                                    lease,
                                    &snapshot,
                                    &operation_id,
                                    &receipt,
                                )
                                .await?;
                            receipt.home().receipt.clone()
                        }
                        Err(_) => homes
                            .prepare(&original.request)
                            .await
                            .map_err(|_| AllocationError::Unavailable)?,
                    }
                } else {
                    homes
                        .prepare(&original.request)
                        .await
                        .map_err(|_| AllocationError::Unavailable)?
                };
                home.bind(receipt.clone())
                    .map_err(|_| AllocationError::IdentityMismatch)?;
                provenance = Some(original.bind(receipt)?);
                (Some(home), ChannelProvisioningPhase::Home)
            } else {
                let attachment = home.attachment().ok_or(AllocationError::IdentityMismatch)?;
                homes
                    .verify(&attachment)
                    .await
                    .map_err(|_| AllocationError::Unavailable)?;
                (None, ChannelProvisioningPhase::Runtime)
            }
        } else if sandbox.lifecycle.home.is_none() {
            if let Some(policy) = &snapshot.state.settings.home_policy {
                let home = SandboxHome::reserve(&sandbox.id, policy.clone())
                    .map_err(|_| AllocationError::Corrupt)?;
                let homes = self
                    .dependencies
                    .adapter
                    .home_volumes()
                    .ok_or(AllocationError::AuthorityUnavailable)?;
                if let Some(scope) = homes
                    .allocation_scope()
                    .filter(|_| snapshot.state.revision == 1)
                {
                    let original = ChannelHomeProvenance::reserve(&snapshot, &home, scope)?;
                    // Commit the conservative dispatch debt with the request.
                    // A crash before the call is indistinguishable from a lost
                    // response, and subsequent receipts never erase that debt.
                    prepare_evidence = Some(ChannelHomePrepareEvidence::capture(&original)?);
                    provenance = Some(original);
                }
                // The fresh request and optional scope provenance commit before
                // prepare. Unsupported adapters keep the existing retained path.
                (Some(home), ChannelProvisioningPhase::Home)
            } else {
                (None, ChannelProvisioningPhase::Runtime)
            }
        } else {
            // Accepted legacy provisioning stays recoverable. Never backfill
            // original provenance from this already-observed canonical home.
            let next = crate::sandbox::home::next_home(
                self.dependencies.adapter.as_ref(),
                &sandbox.id,
                sandbox.lifecycle.home.as_deref(),
                snapshot.state.settings.home_policy.as_ref(),
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
            let phase = if next.is_some() {
                ChannelProvisioningPhase::Home
            } else {
                ChannelProvisioningPhase::Runtime
            };
            (next, phase)
        };
        if let Some(home) = next {
            sandbox.lifecycle.home = Some(Box::new(home));
        }
        self.dependencies
            .credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.dependencies
            .allocations
            .commit_channel_home_provisioning(
                authority,
                lease,
                &snapshot,
                sandbox,
                phase,
                provenance,
                prepare_evidence,
            )
            .await
    }
}
