use super::*;
use crate::sandbox::channel_renewal::model::{
    credential_id, ChannelRenewalState, RenewalClaim, RenewalPhase,
};

pub(super) fn claim(
    snapshot: &mut ChannelRenewalSnapshot,
    holder: &str,
    now: SystemTime,
    lease: &BridgeControlPlaneLease,
    mint_intent: Option<&crate::auth::keys::bridge::mint_intent::BridgeSandboxMintIntent>,
) -> Result<(), AllocationError> {
    if holder.trim().is_empty() || holder.len() > 128 {
        return Err(AllocationError::IdentityMismatch);
    }
    let now_ms = now_ms(now)?;
    if snapshot
        .renewal
        .as_ref()
        .is_some_and(|state| state.claim.expires_at_ms > now_ms)
    {
        return Err(AllocationError::LeaseUnavailable);
    }
    let remaining = lease
        .deadline()
        .saturating_duration_since(tokio::time::Instant::now())
        .min(std::time::Duration::from_secs(60));
    if remaining.is_zero() {
        return Err(AllocationError::AuthorityUnavailable);
    }
    let fence = snapshot.sandbox.work.next_fence;
    snapshot.sandbox.work.next_fence = fence.checked_add(1).ok_or(AllocationError::Corrupt)?;
    let expires_at_ms = now_ms
        .checked_add(i64::try_from(remaining.as_millis()).map_err(|_| AllocationError::Corrupt)?)
        .ok_or(AllocationError::Corrupt)?
        / 1000
        * 1000;
    if expires_at_ms <= now_ms {
        return Err(AllocationError::AuthorityUnavailable);
    }
    let claim = RenewalClaim {
        holder: holder.into(),
        fence,
        expires_at_ms,
    };
    snapshot.sandbox.work.lease = Some(layer2_core::sandbox::SandboxWorkLease {
        holder: claim.holder.clone(),
        fence,
        expires_at: claim_expiry(expires_at_ms)?,
    });
    let revision = snapshot
        .renewal
        .as_ref()
        .map_or_else(
            || {
                snapshot
                    .lifecycle_credential
                    .as_ref()
                    .map_or(Some(1), |state| state.renewal_revision.checked_add(1))
            },
            |state| state.revision.checked_add(1),
        )
        .ok_or(AllocationError::Corrupt)?;
    if snapshot
        .renewal
        .as_ref()
        .is_none_or(|state| state.phase == RenewalPhase::Complete)
    {
        let previous = snapshot
            .sandbox
            .credential
            .current
            .clone()
            .ok_or(AllocationError::Corrupt)?;
        let generation = previous
            .generation
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        let intent = mint_intent.ok_or(AllocationError::Corrupt)?;
        intent
            .require_binding(
                lease,
                &snapshot.provisioning.settings.deployment_id,
                &credential_id(&snapshot.sandbox.id, generation),
            )
            .map_err(|_| AllocationError::IdentityMismatch)?;
        snapshot.renewal = Some(ChannelRenewalState {
            schema_version: 1,
            allocation_id: snapshot.allocation.id.clone(),
            revision,
            phase: RenewalPhase::MintStarted,
            generation,
            credential_id: credential_id(&snapshot.sandbox.id, generation),
            previous_credential_id: if previous.generation == 1 {
                snapshot
                    .provisioning
                    .mint_credential_id
                    .clone()
                    .ok_or(AllocationError::Corrupt)?
            } else {
                credential_id(&snapshot.sandbox.id, previous.generation)
            },
            previous,
            replacement: None,
            mint_intent: Some(intent.clone()),
            claim,
        });
    } else {
        let state = snapshot.renewal.as_mut().ok_or(AllocationError::Corrupt)?;
        state.claim = claim;
        state.revision = revision;
    }
    Ok(())
}

pub(super) fn claim_expiry(milliseconds: i64) -> Result<String, AllocationError> {
    let seconds = u64::try_from(milliseconds / 1000).map_err(|_| AllocationError::Corrupt)?;
    Ok(layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds),
    ))
}
