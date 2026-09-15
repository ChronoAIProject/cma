use super::{ChannelProvisioningPhase::*, ChannelProvisioningSnapshot};
use layer2_core::{bridge::allocation::AllocationError, sandbox::SandboxStatus};

impl ChannelProvisioningSnapshot {
    pub(super) fn validate_progress(&self) -> Result<(), AllocationError> {
        let sandbox = &self.sandbox;
        let credential = &sandbox.credential;
        let before_key = credential.key_generation == 0
            && credential.current.is_none()
            && credential.pending.is_none()
            && !credential.delivered;
        let has_runtime = sandbox.runtime.as_ref().is_some_and(|runtime| {
            !runtime.runtime_id.trim().is_empty()
                && runtime.runtime_version == self.state.settings.runtime_version
                && runtime.provision_generation == Some(1)
                && sandbox.lifecycle.runtime_generation_high_water == 1
        });
        let phase_valid = match self.state.phase {
            Home | Runtime => {
                before_key
                    && sandbox.runtime.is_none()
                    && sandbox.status == SandboxStatus::Provisioning
                    && sandbox.lifecycle.runtime_generation_high_water == 0
            }
            Install | Mint | MintStarted => {
                before_key && has_runtime && sandbox.status == SandboxStatus::Installing
            }
            Deliver => {
                has_runtime
                    && sandbox.status == SandboxStatus::Installing
                    && credential.key_generation == 1
                    && credential
                        .pending
                        .as_ref()
                        .is_some_and(|key| key.generation == 1)
                    && credential.current.is_none()
                    && !credential.delivered
            }
            WorkspaceAdmission | WorkspacePrepare | WorkspaceSetup | AgentAdmission
            | AgentStart | Ready => {
                has_runtime
                    && credential.key_generation == 1
                    && credential
                        .current
                        .as_ref()
                        .is_some_and(|key| key.generation == 1)
                    && credential.pending.is_none()
                    && credential.delivered
                    && sandbox.status
                        == if self.state.phase == Ready {
                            SandboxStatus::Active
                        } else {
                            SandboxStatus::Installing
                        }
            }
        };
        if !phase_valid
            || !credential.service_grants.is_empty()
            || !credential.revoke_debt.is_empty()
            || (self.state.revision == 1
                && (self.state.phase != Home || self.state.last_receipt_digest.is_some()))
            || (self.state.revision > 1 && self.state.last_receipt_digest.is_none())
            || self.state.mint_credential_id.as_ref().is_some_and(|id| {
                id.len() > 128
                    || id.is_empty()
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
        {
            return Err(AllocationError::Corrupt);
        }
        if self.state.phase != Home {
            match (
                &self.state.settings.home_policy,
                sandbox.lifecycle.home.as_deref(),
            ) {
                (Some(policy), Some(home))
                    if home.request.policy == *policy && home.attachment().is_some() => {}
                (None, None) => {}
                _ => return Err(AllocationError::Corrupt),
            }
        }
        Ok(())
    }
}
