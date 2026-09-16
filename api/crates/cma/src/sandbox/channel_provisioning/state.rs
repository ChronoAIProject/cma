use layer2_core::bridge::allocation::{AllocationRecord, ChannelSandboxSidecar};
use layer2_core::sandbox::Sandbox;
use serde::{Deserialize, Serialize};
mod validation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelProvisioningSettings {
    pub deployment_id: String,
    pub runtime_version: String,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub home_policy: Option<layer3_adapter::HomeVolumePolicy>,
}

impl ChannelProvisioningSettings {
    pub fn validate(&self) -> Result<(), layer2_core::bridge::allocation::AllocationError> {
        use layer2_core::bridge::allocation::AllocationError;
        if self.deployment_id.trim().is_empty()
            || self.deployment_id.len() > 128
            || self.runtime_version.trim().is_empty()
            || self.runtime_version.len() > 512
        {
            return Err(AllocationError::Corrupt);
        }
        self.execution
            .validate()
            .map_err(|_| AllocationError::Corrupt)?;
        if let Some(policy) = &self.home_policy {
            policy.validate().map_err(|_| AllocationError::Corrupt)?;
        }
        Ok(())
    }
}

/// Progress remains outside the old aggregate work discriminator. Every stage
/// names one idempotent provider operation; no old worker can claim it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChannelProvisioningPhase {
    Home,
    Runtime,
    Install,
    Mint,
    MintStarted,
    Deliver,
    WorkspaceAdmission,
    WorkspacePrepare,
    WorkspaceSetup,
    AgentAdmission,
    AgentStart,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelProvisioningState {
    pub schema_version: u16,
    pub allocation_id: String,
    pub phase: ChannelProvisioningPhase,
    pub revision: u64,
    pub last_receipt_digest: Option<String>,
    pub settings: ChannelProvisioningSettings,
    /// The exact external key name is persisted before its single mint POST.
    pub mint_credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mint_intent: Option<crate::auth::keys::bridge::mint_intent::BridgeSandboxMintIntent>,
}

impl ChannelProvisioningState {
    pub fn initial(allocation: &AllocationRecord, settings: ChannelProvisioningSettings) -> Self {
        Self {
            schema_version: 1,
            allocation_id: allocation.id.clone(),
            phase: ChannelProvisioningPhase::Home,
            revision: 1,
            last_receipt_digest: None,
            settings,
            mint_credential_id: None,
            mint_intent: None,
        }
    }
    pub fn next(
        &self,
        phase: ChannelProvisioningPhase,
        receipt: &impl Serialize,
    ) -> Result<Self, layer2_core::bridge::allocation::AllocationError> {
        use layer2_core::bridge::allocation::AllocationError;
        use ChannelProvisioningPhase::*;
        if !matches!(
            (self.phase, phase),
            (Home, Home | Runtime)
                | (Runtime, Install)
                | (Install, Mint)
                | (Mint, MintStarted)
                | (MintStarted, Deliver)
                | (Deliver, WorkspaceAdmission)
                | (WorkspaceAdmission, WorkspacePrepare)
                | (WorkspacePrepare, WorkspaceSetup)
                | (WorkspaceSetup, AgentAdmission)
                | (AgentAdmission, AgentStart)
                | (AgentStart, Ready)
        ) {
            return Err(AllocationError::InvalidTransition);
        }
        Ok(Self {
            schema_version: 1,
            allocation_id: self.allocation_id.clone(),
            phase,
            revision: self
                .revision
                .checked_add(1)
                .ok_or(AllocationError::Corrupt)?,
            last_receipt_digest: Some(
                layer2_core::profiles::content_digest(receipt)
                    .map_err(|_| AllocationError::Corrupt)?,
            ),
            settings: self.settings.clone(),
            mint_credential_id: self.mint_credential_id.clone(),
            mint_intent: self.mint_intent.clone(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct ChannelProvisioningSnapshot {
    pub allocation: AllocationRecord,
    pub sandbox: Sandbox,
    pub creation: ChannelSandboxSidecar,
    pub state: ChannelProvisioningState,
    pub home_provenance: Option<super::home_provenance::ChannelHomeProvenance>,
    pub home_prepare_evidence: Option<super::home_prepare_evidence::ChannelHomePrepareEvidence>,
}

impl ChannelProvisioningSnapshot {
    pub fn validate(&self) -> Result<(), layer2_core::bridge::allocation::AllocationError> {
        use layer2_core::bridge::allocation::AllocationError;
        self.creation
            .require_creation(&self.allocation, &self.sandbox)?;
        self.sandbox
            .validate()
            .map_err(|_| AllocationError::Corrupt)?;
        self.state.settings.validate()?;
        if let Some(provenance) = &self.home_provenance {
            provenance.validate(self)?;
        }
        if let Some(evidence) = &self.home_prepare_evidence {
            evidence.validate(self)?;
        }
        if let Some(intent) = &self.state.mint_intent {
            intent.validate().map_err(|_| AllocationError::Corrupt)?;
            if self.state.mint_credential_id.is_none() {
                return Err(AllocationError::Corrupt);
            }
        }
        if self.state.schema_version != 1
            || self.state.allocation_id != self.allocation.id
            || self.state.revision == 0
            || self.sandbox.work.step != layer2_core::sandbox::SandboxWorkStep::None
            || self.sandbox.work.lease.is_some()
            || self.sandbox.work.execution.is_some()
            || self.sandbox.retired_at.is_some()
            || !matches!(
                self.sandbox.status,
                layer2_core::sandbox::SandboxStatus::Provisioning
                    | layer2_core::sandbox::SandboxStatus::Installing
                    | layer2_core::sandbox::SandboxStatus::Active
            )
            || (matches!(
                self.state.phase,
                ChannelProvisioningPhase::MintStarted
                    | ChannelProvisioningPhase::Deliver
                    | ChannelProvisioningPhase::WorkspaceAdmission
                    | ChannelProvisioningPhase::WorkspacePrepare
                    | ChannelProvisioningPhase::WorkspaceSetup
                    | ChannelProvisioningPhase::AgentAdmission
                    | ChannelProvisioningPhase::AgentStart
                    | ChannelProvisioningPhase::Ready
            ) != self.state.mint_credential_id.is_some())
            || self
                .state
                .last_receipt_digest
                .as_ref()
                .is_some_and(|value| {
                    value.len() != 64
                        || !value
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
        {
            return Err(AllocationError::Corrupt);
        }
        self.validate_progress()
    }
}
