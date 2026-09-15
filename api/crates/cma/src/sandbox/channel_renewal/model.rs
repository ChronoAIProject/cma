//! Renewal authority stays separate from the immutable allocation receipt and
//! from ordinary owner-token lifecycle issuance.
use crate::sandbox::channel_provisioning::state::{
    ChannelProvisioningPhase, ChannelProvisioningState,
};
use layer2_core::{
    bridge::allocation::{
        AllocationError, AllocationRecord, AllocationState, ChannelSandboxSidecar,
    },
    sandbox::{Sandbox, SandboxCredentialAuthority, SandboxStatus, SandboxWorkStep},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RenewalPhase {
    MintStarted,
    Deliver,
    Revoke,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenewalClaim {
    pub holder: String,
    pub fence: u64,
    pub expires_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelRenewalState {
    pub schema_version: u16,
    pub allocation_id: String,
    pub revision: u64,
    pub phase: RenewalPhase,
    pub generation: u64,
    pub credential_id: String,
    pub previous_credential_id: String,
    pub previous: SandboxCredentialAuthority,
    pub replacement: Option<SandboxCredentialAuthority>,
    pub claim: RenewalClaim,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ChannelRenewalSnapshot {
    pub allocation: AllocationRecord,
    pub sandbox: Sandbox,
    pub creation: ChannelSandboxSidecar,
    pub provisioning: ChannelProvisioningState,
    pub renewal: Option<ChannelRenewalState>,
    pub lifecycle_credential: Option<
        crate::sandbox::channel_lifecycle_credentials::model::ChannelLifecycleCredentialState,
    >,
}

impl ChannelRenewalSnapshot {
    pub fn validate(&self) -> Result<(), AllocationError> {
        self.sandbox
            .validate()
            .map_err(|_| AllocationError::Corrupt)?;
        self.creation
            .require_creation(&self.allocation, &self.sandbox)?;
        self.provisioning.settings.validate()?;
        if let Some(state) = &self.lifecycle_credential {
            state.validate()?;
            if state.phase
                != crate::sandbox::channel_lifecycle_credentials::model::CredentialPhase::Complete
                || state.allocation_id != self.allocation.id
                || self.sandbox.runtime.as_ref() != Some(&state.runtime)
            {
                return Err(AllocationError::IdentityMismatch);
            }
        }
        if !matches!(
            self.allocation.state,
            AllocationState::Ready | AllocationState::Retired
        ) || self.allocation.receipt.is_none()
            || self.provisioning.phase != ChannelProvisioningPhase::Ready
            || self.provisioning.allocation_id != self.allocation.id
            || self.provisioning.mint_credential_id.is_none()
            || self.sandbox.status != SandboxStatus::Active
            || self.sandbox.runtime.is_none()
            || self.sandbox.retired_at.is_some()
            || self.sandbox.lifecycle.operation.is_some()
            || self.sandbox.lifecycle.checkpoint_capture.is_some()
            || self.sandbox.work.step != SandboxWorkStep::None
            || !self.sandbox.credential.delivered
            || self.sandbox.credential.current.is_none()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let Some(state) = &self.renewal else {
            if self.sandbox.credential.key_generation != 1
                && self.lifecycle_credential.as_ref().is_none_or(|state| {
                    state
                        .require_completed_key(&self.allocation, &self.sandbox)
                        .is_err()
                })
                || self.sandbox.credential.pending.is_some()
                || !self.sandbox.credential.revoke_debt.is_empty()
                || self.sandbox.work.lease.is_some()
            {
                return Err(AllocationError::IdentityMismatch);
            }
            return Ok(());
        };
        if state.schema_version != 1
            || state.allocation_id != self.allocation.id
            || state.revision == 0
            || state.generation < 2
            || state.previous.generation.checked_add(1) != Some(state.generation)
            || state.credential_id != credential_id(&self.sandbox.id, state.generation)
            || state.previous_credential_id
                != if state.previous.generation == 1 {
                    self.provisioning
                        .mint_credential_id
                        .clone()
                        .ok_or(AllocationError::Corrupt)?
                } else {
                    credential_id(&self.sandbox.id, state.previous.generation)
                }
            || state.claim.fence == 0
            || state.claim.holder.is_empty()
            || state.claim.holder.len() > 128
            || state.claim.expires_at_ms <= 0
        {
            return Err(AllocationError::Corrupt);
        }
        let credential = &self.sandbox.credential;
        if state.phase == RenewalPhase::Complete {
            if self.sandbox.work.lease.is_some() {
                return Err(AllocationError::Corrupt);
            }
        } else {
            let work = self
                .sandbox
                .work
                .lease
                .as_ref()
                .ok_or(AllocationError::Corrupt)?;
            let expiry = mongodb::bson::DateTime::parse_rfc3339_str(&work.expires_at)
                .map_err(|_| AllocationError::Corrupt)?
                .timestamp_millis();
            if work.holder != state.claim.holder
                || work.fence != state.claim.fence
                || expiry != state.claim.expires_at_ms
                || self.sandbox.work.next_fence <= work.fence
            {
                return Err(AllocationError::Corrupt);
            }
        }
        let replacement = state.replacement.as_ref();
        let valid = match state.phase {
            RenewalPhase::MintStarted => {
                replacement.is_none()
                    && credential.current.as_ref() == Some(&state.previous)
                    && credential.key_generation == state.previous.generation
                    && credential.pending.is_none()
                    && credential.revoke_debt.is_empty()
            }
            RenewalPhase::Deliver => {
                replacement.is_some_and(|key| key.generation == state.generation)
                    && credential.key_generation == state.generation
                    && credential.pending.as_ref() == replacement
                    && credential.current.as_ref() == Some(&state.previous)
                    && credential.revoke_debt.is_empty()
            }
            RenewalPhase::Revoke => {
                replacement.is_some_and(|key| key.generation == state.generation)
                    && credential.key_generation == state.generation
                    && credential.current.as_ref() == replacement
                    && credential.pending.is_none()
                    && credential.revoke_debt.len() == 1
                    && credential.revoke_debt[0].provider_key_id == state.previous.provider_key_id
            }
            RenewalPhase::Complete => {
                replacement.is_some_and(|key| key.generation == state.generation)
                    && credential.key_generation == state.generation
                    && credential.current.as_ref() == replacement
                    && credential.pending.is_none()
                    && credential.revoke_debt.is_empty()
            }
        };
        if !valid {
            return Err(AllocationError::Corrupt);
        }
        Ok(())
    }

    pub fn due(&self, now_ms: i64) -> Result<bool, AllocationError> {
        self.validate()?;
        if self
            .renewal
            .as_ref()
            .is_some_and(|state| state.phase != RenewalPhase::Complete)
        {
            return Ok(true);
        }
        let current = self
            .sandbox
            .credential
            .current
            .as_ref()
            .ok_or(AllocationError::Corrupt)?;
        let issued = layer2_core::seconds_from_rfc3339(&current.issued_at)
            .ok_or(AllocationError::Corrupt)?;
        let expires = layer2_core::seconds_from_rfc3339(&current.expires_at)
            .ok_or(AllocationError::Corrupt)?;
        let due = issued
            .checked_add(12 * 60 * 60)
            .ok_or(AllocationError::Corrupt)?
            .min(
                expires
                    .checked_sub(5 * 60)
                    .ok_or(AllocationError::Corrupt)?,
            );
        Ok(now_ms
            >= i64::try_from(due)
                .map_err(|_| AllocationError::Corrupt)?
                .checked_mul(1000)
                .ok_or(AllocationError::Corrupt)?)
    }
}

pub fn credential_id(sandbox: &layer2_core::SandboxId, generation: u64) -> String {
    format!("{sandbox}_channel_renew_{generation}")
}

pub(crate) enum RenewalAdmission {
    NotDue,
    /// Only this successful writer may make the single non-idempotent POST.
    Started(ChannelRenewalSnapshot),
    Resume(ChannelRenewalSnapshot),
    MintOutcomeUnknown,
}
