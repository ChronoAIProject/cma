use crate::sandbox::{
    channel_provisioning::state::{ChannelProvisioningPhase, ChannelProvisioningState},
    channel_renewal::model::{credential_id, ChannelRenewalState, RenewalPhase},
};
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord, ChannelSandboxSidecar},
    sandbox::{
        RuntimeBinding, Sandbox, SandboxCredentialAuthority, SandboxLifecycleAuthority,
        SandboxLifecycleOperation, SandboxRevivePhase, SandboxStatus, SandboxWorkStep,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CredentialPhase {
    MintStarted,
    Deliver,
    Revoke,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelLifecycleCredentialState {
    pub schema_version: u16,
    pub allocation_id: String,
    pub operation: SandboxLifecycleAuthority,
    pub runtime: RuntimeBinding,
    pub revision: u64,
    pub renewal_revision: u64,
    pub mint_fence: u64,
    pub phase: CredentialPhase,
    pub generation: u64,
    pub credential_id: String,
    pub previous_credential_id: String,
    pub previous: SandboxCredentialAuthority,
    pub replacement: Option<SandboxCredentialAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mint_intent: Option<crate::auth::keys::bridge::mint_intent::BridgeSandboxMintIntent>,
}
impl ChannelLifecycleCredentialState {
    pub fn require_completed_key(
        &self,
        allocation: &AllocationRecord,
        sandbox: &Sandbox,
    ) -> Result<(), AllocationError> {
        self.validate()?;
        if self.phase != CredentialPhase::Complete
            || self.allocation_id != allocation.id
            || self.replacement.as_ref() != sandbox.credential.current.as_ref()
            || self.generation != sandbox.credential.key_generation
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.revoke_debt.is_empty()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<(), AllocationError> {
        if let Some(intent) = &self.mint_intent {
            intent.validate().map_err(|_| AllocationError::Corrupt)?;
            if let Some(key) = &self.replacement {
                intent
                    .require_material(key)
                    .map_err(|_| AllocationError::IdentityMismatch)?;
            }
        }
        if self.schema_version != 1
            || self.revision == 0
            || self.mint_fence == 0
            || self.allocation_id.is_empty()
            || self.operation.operation_id.is_empty()
            || self.operation.generation == 0
            || self.runtime.provision_generation.is_none()
            || self.generation < 2
            || self.previous.generation.checked_add(1) != Some(self.generation)
            || self.credential_id.is_empty()
            || self.previous_credential_id.is_empty()
            || match self.phase {
                CredentialPhase::MintStarted => self.replacement.is_some(),
                _ => self.replacement.as_ref().is_none_or(|key| {
                    key.generation != self.generation
                        || key.provider_key_id == self.previous.provider_key_id
                }),
            }
        {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChannelLifecycleSnapshot {
    pub allocation: AllocationRecord,
    pub sandbox: Sandbox,
    pub creation: ChannelSandboxSidecar,
    pub provisioning: ChannelProvisioningState,
    pub renewal: Option<ChannelRenewalState>,
    pub credential: Option<ChannelLifecycleCredentialState>,
}
impl ChannelLifecycleSnapshot {
    pub fn validate(&self) -> Result<(), AllocationError> {
        self.sandbox
            .validate()
            .map_err(|_| AllocationError::Corrupt)?;
        self.creation
            .require_creation(&self.allocation, &self.sandbox)?;
        self.provisioning.settings.validate()?;
        if self.allocation.receipt.is_none()
            || self.provisioning.phase != ChannelProvisioningPhase::Ready
            || self.provisioning.allocation_id != self.allocation.id
            || self.provisioning.mint_credential_id.is_none()
            || self.sandbox.retired_at.is_some()
            || self.sandbox.lifecycle.checkpoint_capture.is_some()
            || self.sandbox.credential.current.is_none()
            || !self.sandbox.credential.delivered
            || self
                .renewal
                .as_ref()
                .is_some_and(|state| state.phase != RenewalPhase::Complete)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        match self.sandbox.status {
            SandboxStatus::Paused
                if self.sandbox.lifecycle.operation.is_none()
                    && self.sandbox.runtime.is_none()
                    && self.sandbox.work.lease.is_none() => {}
            SandboxStatus::Reviving
                if matches!(
                    self.sandbox.lifecycle.operation,
                    Some(SandboxLifecycleOperation::Revive { .. })
                ) && self.sandbox.work.step == SandboxWorkStep::RunLifecycleOperation => {}
            _ => return Err(AllocationError::IdentityMismatch),
        }
        if let Some(state) = &self.credential {
            state.validate()?;
            if state.allocation_id != self.allocation.id
                || state.credential_id != credential_id(&self.sandbox.id, state.generation)
                || state.previous_credential_id != self.name_for(state.previous.generation)?
            {
                return Err(AllocationError::IdentityMismatch);
            }
            let same_operation = self
                .sandbox
                .lifecycle
                .operation
                .as_ref()
                .is_some_and(|op| op.authority() == &state.operation);
            if same_operation {
                if self.sandbox.runtime.as_ref() != Some(&state.runtime) {
                    return Err(AllocationError::IdentityMismatch);
                }
                let c = &self.sandbox.credential;
                let valid = match state.phase {
                    CredentialPhase::MintStarted => {
                        c.current.as_ref() == Some(&state.previous)
                            && c.pending.is_none()
                            && c.key_generation == state.previous.generation
                            && c.revoke_debt.is_empty()
                    }
                    CredentialPhase::Deliver => {
                        c.current.as_ref() == Some(&state.previous)
                            && c.pending.as_ref() == state.replacement.as_ref()
                            && c.key_generation == state.generation
                            && c.revoke_debt.is_empty()
                    }
                    CredentialPhase::Revoke => {
                        c.current.as_ref() == state.replacement.as_ref()
                            && c.pending.is_none()
                            && c.key_generation == state.generation
                            && c.revoke_debt.len() == 1
                            && c.revoke_debt[0].provider_key_id == state.previous.provider_key_id
                    }
                    CredentialPhase::Complete => {
                        c.current.as_ref() == state.replacement.as_ref()
                            && c.pending.is_none()
                            && c.key_generation == state.generation
                            && c.revoke_debt.is_empty()
                    }
                };
                if !valid || (state.phase != CredentialPhase::Complete && !self.is_delivery()) {
                    return Err(AllocationError::IdentityMismatch);
                }
                return Ok(());
            }
            if state.phase != CredentialPhase::Complete {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        self.require_prior_key()
    }
    pub fn is_delivery(&self) -> bool {
        matches!(
            self.sandbox.lifecycle.operation,
            Some(SandboxLifecycleOperation::Revive {
                phase: SandboxRevivePhase::CredentialDelivery,
                ..
            })
        )
    }
    pub fn current_attempt(&self) -> Option<&ChannelLifecycleCredentialState> {
        self.credential.as_ref().filter(|state| {
            self.sandbox
                .lifecycle
                .operation
                .as_ref()
                .is_some_and(|op| op.authority() == &state.operation)
        })
    }
    pub fn name_for(&self, generation: u64) -> Result<String, AllocationError> {
        if generation == 1 {
            self.provisioning
                .mint_credential_id
                .clone()
                .ok_or(AllocationError::Corrupt)
        } else {
            Ok(credential_id(&self.sandbox.id, generation))
        }
    }
    fn require_prior_key(&self) -> Result<(), AllocationError> {
        if self.sandbox.credential.pending.is_some()
            || !self.sandbox.credential.revoke_debt.is_empty()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        if let Some(state) = &self.renewal {
            if state.phase == RenewalPhase::Complete
                && state.allocation_id == self.allocation.id
                && state.replacement.as_ref() == self.sandbox.credential.current.as_ref()
                && state.generation == self.sandbox.credential.key_generation
            {
                return Ok(());
            }
        }
        if let Some(state) = &self.credential {
            return state.require_completed_key(&self.allocation, &self.sandbox);
        }
        if self.sandbox.credential.key_generation != 1 {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}
