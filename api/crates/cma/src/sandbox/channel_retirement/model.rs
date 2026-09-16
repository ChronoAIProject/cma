use crate::sandbox::{
    channel_lifecycle_credentials::model::ChannelLifecycleCredentialState,
    channel_provisioning::{
        home_prepare_evidence::ChannelHomePrepareEvidence,
        home_provenance::ChannelHomeProvenance,
        state::{ChannelProvisioningPhase, ChannelProvisioningState},
    },
    channel_renewal::model::ChannelRenewalState,
};
use layer2_core::{
    bridge::allocation::{AllocationError, AllocationRecord, ChannelSandboxSidecar},
    sandbox::{
        RuntimeBinding, Sandbox, SandboxCredentialAuthority, SandboxCredentialState,
        SandboxWorkStep,
    },
};
use serde::{Deserialize, Serialize};
mod keys;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetirementKey {
    pub authority: SandboxCredentialAuthority,
    pub credential_id: String,
    pub revoked: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RetirementPhase {
    StopIntent,
    Revoke,
    CredentialsCleared,
    RuntimeIntent,
    RuntimeGone,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelRetirementState {
    pub schema_version: u16,
    pub allocation_id: String,
    pub sandbox_created_at: String,
    pub tree_fingerprint: String,
    pub original_credential: SandboxCredentialState,
    pub runtime: Option<RuntimeBinding>,
    pub keys: Vec<RetirementKey>,
    pub phase: RetirementPhase,
}
pub(crate) struct OwnedTree {
    pub fingerprint: String,
    pub agent_live: bool,
}
pub(crate) struct ChannelRetirementSnapshot {
    pub allocation: AllocationRecord,
    pub sandbox: Sandbox,
    pub creation: ChannelSandboxSidecar,
    pub provisioning: ChannelProvisioningState,
    pub home_provenance: Option<ChannelHomeProvenance>,
    pub home_prepare_evidence: Option<ChannelHomePrepareEvidence>,
    pub renewal: Option<ChannelRenewalState>,
    pub lifecycle: Option<ChannelLifecycleCredentialState>,
    pub state: Option<ChannelRetirementState>,
    pub retention: Option<super::retention::ChannelStorageRetention>,
    pub retention_audit: Option<super::assessment::RetentionAuditIdentity>,
    pub history: crate::store::channel_history::ChannelHistoryAssessment,
    pub tree: OwnedTree,
}
impl ChannelRetirementSnapshot {
    pub fn validate(&self) -> Result<(), AllocationError> {
        self.creation
            .require_creation(&self.allocation, &self.sandbox)?;
        self.provisioning.settings.validate()?;
        // Missing legacy evidence is assessed only at the final retained
        // boundary. Present but contradictory evidence is never accepted.
        if let Some(original) = &self.home_provenance {
            original.validate_retained(
                &self.allocation,
                &self.sandbox,
                &self.creation,
                &self.provisioning.settings,
            )?;
        }
        if let Some(evidence) = &self.home_prepare_evidence {
            evidence.validate_original(
                self.home_provenance
                    .as_ref()
                    .ok_or(AllocationError::IdentityMismatch)?,
            )?;
        }
        if self.provisioning.schema_version != 1
            || self.provisioning.revision == 0
            || self.provisioning.allocation_id != self.allocation.id
            || self.provisioning.phase != ChannelProvisioningPhase::Ready
            || self.provisioning.mint_credential_id.is_none()
            || self.allocation.receipt.is_none()
            || self.sandbox.retired_at.is_some()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let Some(state) = &self.state else {
            if self.retention.is_some() {
                return Err(AllocationError::IdentityMismatch);
            }
            if self.sandbox.work.step != SandboxWorkStep::StopAgents {
                return Err(AllocationError::InvalidTransition);
            }
            self.require_runtime_accounted_for(self.sandbox.runtime.as_ref())?;
            self.initial_keys()?;
            return Ok(());
        };
        if state.schema_version != 1
            || state.allocation_id != self.allocation.id
            || state.sandbox_created_at != self.sandbox.created_at
            || state.tree_fingerprint != self.tree.fingerprint
            || state.keys.is_empty()
            || !state.original_credential.validate()
        {
            return Err(AllocationError::IdentityMismatch);
        }
        self.require_runtime_accounted_for(state.runtime.as_ref())?;
        let mut original =
            self.original_snapshot(&state.original_credential, state.runtime.clone());
        original.state = None;
        let expected = original.initial_keys()?;
        if expected.len() != state.keys.len()
            || expected
                .iter()
                .zip(&state.keys)
                .any(|(a, b)| a.authority != b.authority || a.credential_id != b.credential_id)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        let mut credential = state.original_credential.clone();
        for key in state.keys.iter().filter(|key| key.revoked) {
            remove_key(&mut credential, &key.authority.provider_key_id);
        }
        if credential != self.sandbox.credential {
            return Err(AllocationError::IdentityMismatch);
        }
        let complete = state.keys.iter().all(|key| key.revoked);
        let valid = match state.phase {
            RetirementPhase::StopIntent => {
                self.sandbox.work.step == SandboxWorkStep::StopAgents
                    && state.keys.iter().all(|key| !key.revoked)
            }
            RetirementPhase::Revoke => {
                self.sandbox.work.step == SandboxWorkStep::RevokeRetirementCredentials
                    && !self.tree.agent_live
                    && !complete
            }
            RetirementPhase::CredentialsCleared | RetirementPhase::RuntimeIntent => {
                self.sandbox.work.step == SandboxWorkStep::TeardownRuntime
                    && !self.tree.agent_live
                    && complete
            }
            RetirementPhase::RuntimeGone => {
                self.sandbox.work.step == SandboxWorkStep::MarkRetired
                    && !self.tree.agent_live
                    && complete
                    && self.sandbox.runtime.is_none()
            }
        };
        if !valid
            || (state.phase != RetirementPhase::RuntimeGone
                && self.sandbox.runtime != state.runtime)
        {
            return Err(AllocationError::IdentityMismatch);
        }
        if let Some(retention) = &self.retention {
            retention.validate(self)?;
        }
        Ok(())
    }
    fn require_runtime_accounted_for(
        &self,
        runtime: Option<&RuntimeBinding>,
    ) -> Result<(), AllocationError> {
        // Retirement cancels lifecycle work and clears its runtime reservation.
        // None cannot prove that a previously dispatched create cannot finish.
        // Keep paused/unbound roots held until exact absence has its own receipt.
        let runtime = runtime.ok_or(AllocationError::AuthorityUnavailable)?;
        if self.sandbox.lifecycle.runtime_provision.is_some()
            || self.sandbox.lifecycle.runtime_generation_high_water == 0
            || runtime.provision_generation
                != Some(self.sandbox.lifecycle.runtime_generation_high_water)
        {
            return Err(AllocationError::AuthorityUnavailable);
        }
        Ok(())
    }
    pub fn initial_state(&self) -> Result<ChannelRetirementState, AllocationError> {
        Ok(ChannelRetirementState {
            schema_version: 1,
            allocation_id: self.allocation.id.clone(),
            sandbox_created_at: self.sandbox.created_at.clone(),
            tree_fingerprint: self.tree.fingerprint.clone(),
            original_credential: self.sandbox.credential.clone(),
            runtime: self.sandbox.runtime.clone(),
            keys: self.initial_keys()?,
            phase: RetirementPhase::StopIntent,
        })
    }
    fn original_snapshot(
        &self,
        credential: &SandboxCredentialState,
        runtime: Option<RuntimeBinding>,
    ) -> Self {
        let mut sandbox = self.sandbox.clone();
        sandbox.credential = credential.clone();
        sandbox.runtime = runtime;
        Self {
            allocation: self.allocation.clone(),
            sandbox,
            creation: self.creation.clone(),
            provisioning: self.provisioning.clone(),
            home_provenance: self.home_provenance.clone(),
            home_prepare_evidence: self.home_prepare_evidence.clone(),
            renewal: self.renewal.clone(),
            lifecycle: self.lifecycle.clone(),
            state: None,
            retention: None,
            retention_audit: None,
            history: self.history.clone(),
            tree: OwnedTree {
                fingerprint: self.tree.fingerprint.clone(),
                agent_live: self.tree.agent_live,
            },
        }
    }
}
pub(crate) fn remove_key(credential: &mut SandboxCredentialState, id: &str) {
    if credential
        .current
        .as_ref()
        .is_some_and(|key| key.provider_key_id == id)
    {
        credential.current = None;
    }
    if credential
        .pending
        .as_ref()
        .is_some_and(|key| key.provider_key_id == id)
    {
        credential.pending = None;
    }
    credential
        .revoke_debt
        .retain(|debt| debt.provider_key_id != id);
    credential.delivered = credential.current.is_some();
}
