//! Recover only the orphan key created by a retained uncertain request. A
//! revoked orphan remains a held mint; this path never dispatches key creation.
pub(crate) mod authority;
use crate::{
    auth::keys::KeyIssuer,
    sandbox::channel_provisioning::ChannelControlPlaneVerifier,
    store::{
        bridge_allocations::channel_mint_reconciliation::MintReconciliationChange as Change,
        MongoBridgeAllocationStore,
    },
};
use authority::ChannelMintAttempt;
use layer2_core::bridge::allocation::AllocationError;

pub(crate) async fn reconcile(
    allocations: &MongoBridgeAllocationStore,
    credentials: &dyn ChannelControlPlaneVerifier,
    issuer: &KeyIssuer,
    attempt: ChannelMintAttempt<'_>,
) -> Result<(), AllocationError> {
    credentials
        .revalidate_control_plane(attempt.human())
        .await
        .map_err(|_| AllocationError::AuthorityUnavailable)?;
    let receipt = allocations
        .change_channel_mint_reconciliation(&attempt, Change::Read)
        .await?;
    let receipt = match receipt {
        Some(receipt) => receipt,
        None => {
            let Some(observed) = issuer
                .observe_bridge_sandbox_mint(
                    attempt.human(),
                    attempt.intent()?,
                    attempt.generation()?,
                )
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?
            else {
                return Err(AllocationError::Unavailable);
            };
            credentials
                .revalidate_control_plane(attempt.human())
                .await
                .map_err(|_| AllocationError::AuthorityUnavailable)?;
            allocations
                .change_channel_mint_reconciliation(&attempt, Change::Observe(&observed))
                .await?
                .ok_or(AllocationError::Corrupt)?
        }
    };
    if !receipt.revoked {
        credentials
            .revalidate_control_plane(attempt.human())
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        allocations
            .change_channel_mint_reconciliation(&attempt, Change::Read)
            .await?;
        issuer
            .revoke_bridge_sandbox_key(attempt.human(), attempt.credential_id()?, &receipt.key)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        credentials
            .revalidate_control_plane(attempt.human())
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        allocations
            .change_channel_mint_reconciliation(&attempt, Change::Revoke(&receipt.key))
            .await?;
    }
    // No secret was recoverable and the original POST is never replayable.
    Err(AllocationError::Unavailable)
}
