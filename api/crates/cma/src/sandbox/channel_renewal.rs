//! Scoped credential replacement preserves the actual runtime and every Agent
//! session. The ordinary owner SubjectId token issuer cannot enter this lane.
pub(crate) mod model;
use super::{
    channel_provisioning::ChannelControlPlaneVerifier, CredentialMaterial, SandboxCredentialCustody,
};
use crate::{
    auth::keys::{bridge::BridgeSandboxKeyPlan, KeyIssuer, MintedKey},
    bridge::BridgeControlPlaneLease,
    store::{MongoBridgeAllocationStore, MongoSandboxCredentialStore},
};
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError, AllocationRecord},
    sandbox::SandboxCredentialAuthority,
};
use layer3_adapter::{CredentialFrame, RuntimeCredential, SandboxAdapter};
use model::{ChannelRenewalSnapshot, RenewalAdmission, RenewalPhase};
use std::{sync::Arc, time::SystemTime};

pub(crate) struct ChannelCredentialRenewalDriver {
    pub allocations: Arc<MongoBridgeAllocationStore>,
    pub credentials: Arc<dyn ChannelControlPlaneVerifier>,
    pub issuer: Arc<KeyIssuer>,
    pub custody: Arc<MongoSandboxCredentialStore>,
    pub adapter: Arc<dyn SandboxAdapter>,
    pub holder: String,
}

impl ChannelCredentialRenewalDriver {
    pub(crate) async fn run_one(
        &self,
        allocation: &AllocationRecord,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let previous = self
            .allocations
            .current_channel_renewal(authority, lease, allocation)
            .await?;
        if !previous.due(now_ms(now)?)? {
            return Ok(());
        }
        // Read-only preflight precedes the unique POST intent. A failed scope
        // observation cannot strand an attempt that never dispatched a mint.
        let plan = if previous
            .renewal
            .as_ref()
            .is_none_or(|state| state.phase == RenewalPhase::Complete)
        {
            let generation = previous
                .sandbox
                .credential
                .next_generation()
                .ok_or(AllocationError::Corrupt)?;
            Some(
                self.issuer
                    .plan_bridge_sandbox_key(
                        lease,
                        &model::credential_id(&previous.sandbox.id, generation),
                        &previous.sandbox.credential.effective_reaches(),
                    )
                    .await
                    .map_err(|_| AllocationError::AuthorityUnavailable)?,
            )
        } else {
            None
        };
        let intent = plan
            .as_ref()
            .map(BridgeSandboxKeyPlan::mint_intent)
            .transpose()
            .map_err(|_| AllocationError::Corrupt)?;
        let admission = self
            .allocations
            .admit_channel_renewal(
                authority,
                lease,
                &previous,
                (&self.holder, now),
                intent.as_ref(),
            )
            .await?;
        let now = SystemTime::now();
        match admission {
            RenewalAdmission::NotDue => Ok(()),
            RenewalAdmission::MintOutcomeUnknown => Err(AllocationError::Unavailable),
            RenewalAdmission::Reconcile(snapshot) => {
                crate::sandbox::channel_mint_reconciliation::reconcile(&self.allocations, self.credentials.as_ref(), &self.issuer,
                    crate::sandbox::channel_mint_reconciliation::authority::ChannelMintAttempt::Renewal { authority, human: lease, snapshot: &snapshot }).await
            },
            RenewalAdmission::Started(snapshot) => {
                self.mint(
                    authority,
                    lease,
                    &snapshot,
                    plan.ok_or(AllocationError::Corrupt)?,
                    now,
                )
                .await
            }
            RenewalAdmission::Resume(snapshot) => match snapshot
                .renewal
                .as_ref()
                .ok_or(AllocationError::Corrupt)?
                .phase
            {
                RenewalPhase::Deliver => self.deliver(authority, lease, &snapshot, now).await,
                RenewalPhase::Revoke => self.revoke(authority, lease, &snapshot, now).await,
                _ => Err(AllocationError::InvalidTransition),
            },
        }
    }

    async fn mint(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelRenewalSnapshot,
        plan: BridgeSandboxKeyPlan,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        let state = snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?;
        self.revalidate(authority, lease, snapshot).await?;
        let minted = self
            .issuer
            .mint_bridge_sandbox_key(lease, plan)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let material = CredentialMaterial {
            sandbox_id: snapshot.sandbox.id.clone(),
            owner: snapshot.sandbox.owner.clone(),
            authority: SandboxCredentialAuthority {
                generation: state.generation,
                provider_key_id: minted.id,
                provider_state_version: minted.state_version,
                issued_at: layer2_core::rfc3339_at(now),
                expires_at: layer2_core::rfc3339_at(minted.expires_at),
                grant_fingerprint: minted.grant_fingerprint,
            },
            value: minted.value,
        };
        self.revalidate(authority, lease, snapshot).await?;
        self.allocations
            .commit_channel_renewal_mint(authority, lease, snapshot, &self.custody, material, now)
            .await
    }

    async fn deliver(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelRenewalSnapshot,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        let state = snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?;
        let pending = state.replacement.as_ref().ok_or(AllocationError::Corrupt)?;
        let material = self
            .custody
            .get(&snapshot.sandbox.id, pending.generation)
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Corrupt)?;
        if material.owner != snapshot.sandbox.owner
            || material.sandbox_id != snapshot.sandbox.id
            || &material.authority != pending
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let expiry = mongodb::bson::DateTime::parse_rfc3339_str(&pending.expires_at)
            .map_err(|_| AllocationError::Corrupt)?
            .to_system_time();
        if expiry <= now + std::time::Duration::from_secs(300) {
            return Err(AllocationError::AuthorityUnavailable);
        }
        let plan = self
            .issuer
            .plan_bridge_sandbox_key(
                lease,
                &state.credential_id,
                &snapshot.sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let minted = MintedKey {
            id: pending.provider_key_id.clone(),
            value: material.value,
            expires_at: expiry,
            state_version: pending.provider_state_version,
            grant_fingerprint: pending.grant_fingerprint.clone(),
        };
        self.issuer
            .require_bridge_sandbox_material(lease, &plan, &minted)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let runtime = super::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.provisioning.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        self.revalidate(authority, lease, snapshot).await?;
        self.adapter
            .deliver_credential(
                &runtime,
                CredentialFrame {
                    operation_id: format!(
                        "{}:credential:{}",
                        snapshot.allocation.operation_id, pending.generation
                    ),
                    value: RuntimeCredential::new(minted.value.expose()),
                    custody_generation: Some(pending.generation),
                },
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        self.revalidate(authority, lease, snapshot).await?;
        self.allocations
            .commit_channel_renewal_delivery(authority, lease, snapshot, now)
            .await
    }

    async fn revoke(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelRenewalSnapshot,
        now: SystemTime,
    ) -> Result<(), AllocationError> {
        let state = snapshot.renewal.as_ref().ok_or(AllocationError::Corrupt)?;
        self.revalidate(authority, lease, snapshot).await?;
        self.issuer
            .revoke_bridge_sandbox_key(lease, &state.previous_credential_id, &state.previous)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.revalidate(authority, lease, snapshot).await?;
        self.allocations
            .commit_channel_renewal_revocation(authority, lease, snapshot, &self.custody, now)
            .await
    }

    async fn revalidate(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelRenewalSnapshot,
    ) -> Result<(), AllocationError> {
        self.credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.allocations
            .revalidate_channel_renewal(authority, lease, snapshot)
            .await
    }
}

fn now_ms(now: SystemTime) -> Result<i64, AllocationError> {
    now.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or(AllocationError::Unavailable)
}
