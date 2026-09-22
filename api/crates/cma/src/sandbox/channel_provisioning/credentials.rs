use super::{ChannelProvisioningDriver, ChannelProvisioningPhase, ChannelProvisioningSnapshot};
use crate::{
    bridge::BridgeControlPlaneLease,
    sandbox::{CredentialMaterial, SandboxCredentialCustody},
};
use layer2_core::{
    bridge::allocation::{AllocationAuthority, AllocationError},
    sandbox::{SandboxCredentialAuthority, SandboxEventKind},
};
use layer3_adapter::{CredentialFrame, RuntimeCredential};

impl ChannelProvisioningDriver {
    pub(super) async fn mint(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        if snapshot.sandbox.credential.key_generation != 0
            || snapshot.sandbox.credential.pending.is_some()
            || snapshot.sandbox.credential.current.is_some()
        {
            return Err(AllocationError::Corrupt);
        }
        let credential_id = format!(
            "{}_channel_{}",
            snapshot.sandbox.id, snapshot.allocation.lease_fence
        );
        let plan = self
            .dependencies
            .issuer
            .plan_bridge_sandbox_key(
                lease,
                &credential_id,
                &snapshot.sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let mut sandbox = snapshot.sandbox.clone();
        sandbox.revision = sandbox
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        sandbox.updated_at = layer2_core::rfc3339_at(std::time::SystemTime::now());
        let mut state = snapshot
            .state
            .next(ChannelProvisioningPhase::MintStarted, &credential_id)?;
        state.mint_credential_id = Some(credential_id);
        state.mint_intent = Some(plan.mint_intent().map_err(|_| AllocationError::Corrupt)?);
        // This transition retains the exact claim; only its successful writer
        // can dispatch the one POST, and every resumed MintStarted is held.
        let started = self
            .dependencies
            .allocations
            .commit_channel_provisioning(authority, lease, &snapshot, sandbox, state, None)
            .await?;
        self.require_effect(authority, lease, &started).await?;
        let minted = self
            .dependencies
            .issuer
            .mint_bridge_sandbox_key(lease, plan)
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let material = CredentialMaterial {
            source: Default::default(),
            sandbox_id: started.sandbox.id.clone(),
            owner: started.sandbox.owner.clone(),
            authority: SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: minted.id,
                provider_state_version: minted.state_version,
                issued_at: layer2_core::rfc3339_at(std::time::SystemTime::now()),
                expires_at: layer2_core::rfc3339_at(minted.expires_at),
                grant_fingerprint: minted.grant_fingerprint,
            },
            value: minted.value,
        };
        self.dependencies
            .credentials
            .revalidate_control_plane(lease)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let mut sandbox = started.sandbox.clone();
        sandbox.credential.key_generation = 1;
        sandbox.credential.pending = Some(material.authority.clone());
        sandbox.revision = sandbox
            .revision
            .checked_add(1)
            .ok_or(AllocationError::Corrupt)?;
        sandbox.updated_at = layer2_core::rfc3339_at(std::time::SystemTime::now());
        let state = started
            .state
            .next(ChannelProvisioningPhase::Deliver, &material.authority)?;
        self.dependencies
            .allocations
            .commit_channel_minted_key(
                authority,
                lease,
                &started,
                sandbox,
                state,
                self.dependencies.custody.as_ref(),
                material,
            )
            .await
    }

    pub(super) async fn deliver(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: ChannelProvisioningSnapshot,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        self.require_effect(authority, lease, &snapshot).await?;
        let pending = snapshot
            .sandbox
            .credential
            .pending
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        let material = self
            .dependencies
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
        if expiry <= std::time::SystemTime::now() + std::time::Duration::from_secs(300) {
            return Err(AllocationError::AuthorityUnavailable);
        }
        let plan = self
            .dependencies
            .issuer
            .plan_bridge_sandbox_key(
                lease,
                snapshot
                    .state
                    .mint_credential_id
                    .as_deref()
                    .ok_or(AllocationError::Corrupt)?,
                &snapshot.sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let minted = crate::auth::keys::MintedKey {
            id: pending.provider_key_id.clone(),
            value: material.value,
            expires_at: expiry,
            state_version: pending.provider_state_version,
            grant_fingerprint: pending.grant_fingerprint.clone(),
        };
        self.dependencies
            .issuer
            .require_bridge_sandbox_material(lease, &plan, &minted)
            .await
            .map_err(|_| AllocationError::AuthorityUnavailable)?;
        let runtime = crate::sandbox::runtime_attachment::adopt_bound_runtime(
            self.dependencies.adapter.as_ref(),
            &snapshot.sandbox,
            &snapshot.state.settings.deployment_id,
        )
        .await
        .map_err(|_| AllocationError::Unavailable)?;
        self.require_effect(authority, lease, &snapshot).await?;
        self.dependencies
            .adapter
            .deliver_credential(
                &runtime,
                CredentialFrame {
                    operation_id: format!("{}:credential:1", snapshot.allocation.operation_id),
                    value: RuntimeCredential::new(minted.value.expose()),
                    custody_generation: Some(pending.generation),
                },
            )
            .await
            .map_err(|_| AllocationError::Unavailable)?;
        let mut sandbox = snapshot.sandbox.clone();
        sandbox.credential.current = sandbox.credential.pending.take();
        sandbox.credential.delivered = true;
        self.commit(
            authority,
            lease,
            &snapshot,
            sandbox,
            ChannelProvisioningPhase::WorkspaceAdmission,
            pending,
            Some(SandboxEventKind::CredentialChanged {
                key_generation: pending.generation,
            }),
        )
        .await
    }
}
