use super::{
    model::{ChannelLifecycleSnapshot, CredentialPhase},
    ChannelLifecycleCredentialDriver,
};
use crate::{
    auth::keys::MintedKey,
    bridge::provisioning::revival::ChannelRevivalAuthority,
    sandbox::{CredentialMaterial, SandboxCredentialCustody},
    store::bridge_allocations::channel_lifecycle::LifecycleCredentialChange as Change,
};
use layer2_core::{
    bridge::allocation::AllocationError,
    sandbox::{Sandbox, SandboxCredentialAuthority},
};
use layer3_adapter::{CredentialFrame, RuntimeCredential};
use std::time::{Duration, SystemTime};

impl ChannelLifecycleCredentialDriver {
    pub(super) async fn revalidate(
        &self,
        permit: &ChannelRevivalAuthority,
    ) -> Result<ChannelLifecycleSnapshot, AllocationError> {
        self.credentials
            .revalidate_control_plane(permit.human())
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.allocations.current_channel_lifecycle(permit).await
    }
    pub(super) async fn run_credential(&self, sandbox: &Sandbox) -> Result<(), AllocationError> {
        let permit = self
            .authority
            .resolve(sandbox)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let snapshot = self.revalidate(&permit).await?;
        if !snapshot.is_delivery() {
            return Err(AllocationError::InvalidTransition);
        }
        match snapshot.current_attempt().map(|state| state.phase) {
            None => self.mint(permit, &snapshot).await,
            Some(CredentialPhase::MintStarted) => {
                crate::sandbox::channel_mint_reconciliation::reconcile(&self.allocations, self.credentials.as_ref(), &self.issuer,
                    crate::sandbox::channel_mint_reconciliation::authority::ChannelMintAttempt::Revival { permit: &permit, snapshot: &snapshot }).await
            },
            Some(CredentialPhase::Deliver) => self.deliver(&permit, &snapshot).await,
            Some(CredentialPhase::Revoke) => self.revoke(&permit, &snapshot).await,
            Some(CredentialPhase::Complete) => Err(AllocationError::InvalidTransition),
        }
    }
    async fn mint(
        &self,
        permit: ChannelRevivalAuthority,
        snapshot: &ChannelLifecycleSnapshot,
    ) -> Result<(), AllocationError> {
        let generation = snapshot
            .sandbox
            .credential
            .next_generation()
            .ok_or(AllocationError::Corrupt)?;
        // Failed read-only scope preflight cannot create an ambiguous POST.
        let plan = self
            .issuer
            .plan_bridge_sandbox_key(
                permit.human(),
                &snapshot.name_for(generation)?,
                &snapshot.sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.revalidate(&permit).await?;
        let intent = plan.mint_intent().map_err(|_| AllocationError::Corrupt)?;
        let started = self
            .allocations
            .change_channel_lifecycle(&permit, Change::MintIntent(&intent))
            .await?;
        let permit = permit.after_credential_intent(&started.sandbox)?;
        self.revalidate(&permit).await?;
        let now = SystemTime::now();
        let minted = self
            .issuer
            .mint_bridge_sandbox_key(permit.human(), plan)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let material = CredentialMaterial {
            sandbox_id: started.sandbox.id.clone(),
            owner: started.sandbox.owner.clone(),
            authority: SandboxCredentialAuthority {
                generation,
                provider_key_id: minted.id,
                provider_state_version: minted.state_version,
                issued_at: layer2_core::rfc3339_at(now),
                expires_at: layer2_core::rfc3339_at(minted.expires_at),
                grant_fingerprint: minted.grant_fingerprint,
            },
            value: minted.value,
        };
        self.revalidate(&permit).await?;
        self.allocations
            .change_channel_lifecycle(
                &permit,
                Change::Mint {
                    custody: &self.custody,
                    material: &material,
                },
            )
            .await
            .map(|_| ())
    }
    async fn deliver(
        &self,
        permit: &ChannelRevivalAuthority,
        snapshot: &ChannelLifecycleSnapshot,
    ) -> Result<(), AllocationError> {
        let state = snapshot.current_attempt().ok_or(AllocationError::Corrupt)?;
        let pending = state.replacement.as_ref().ok_or(AllocationError::Corrupt)?;
        let material = self
            .custody
            .get(&snapshot.sandbox.id, pending.generation)
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Corrupt)?;
        if material.sandbox_id != snapshot.sandbox.id
            || material.owner != snapshot.sandbox.owner
            || &material.authority != pending
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let expiry = mongodb::bson::DateTime::parse_rfc3339_str(&pending.expires_at)
            .map_err(|_| AllocationError::Corrupt)?
            .to_system_time();
        if expiry <= SystemTime::now() + Duration::from_secs(300) {
            return Err(AllocationError::AuthorityUnavailable);
        }
        let plan = self
            .issuer
            .plan_bridge_sandbox_key(
                permit.human(),
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
            .require_bridge_sandbox_material(permit.human(), &plan, &minted)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.revalidate(permit).await?;
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.provisioning.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        self.revalidate(permit).await?;
        self.adapter
            .deliver_credential(
                &runtime,
                CredentialFrame {
                    operation_id: format!(
                        "{}:runtime:{}:credential:{}",
                        state.operation.operation_id,
                        state
                            .runtime
                            .provision_generation
                            .ok_or(AllocationError::Corrupt)?,
                        state.generation
                    ),
                    value: RuntimeCredential::new(minted.value.expose()),
                    custody_generation: Some(pending.generation),
                },
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        self.revalidate(permit).await?;
        self.allocations
            .change_channel_lifecycle(permit, Change::Deliver)
            .await
            .map(|_| ())
    }
    async fn revoke(
        &self,
        permit: &ChannelRevivalAuthority,
        snapshot: &ChannelLifecycleSnapshot,
    ) -> Result<(), AllocationError> {
        let state = snapshot.current_attempt().ok_or(AllocationError::Corrupt)?;
        self.revalidate(permit).await?;
        self.issuer
            .revoke_bridge_sandbox_key(
                permit.human(),
                &state.previous_credential_id,
                &state.previous,
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        self.revalidate(permit).await?;
        self.allocations
            .change_channel_lifecycle(
                permit,
                Change::Revoke {
                    custody: &self.custody,
                },
            )
            .await
            .map(|_| ())
    }
}
