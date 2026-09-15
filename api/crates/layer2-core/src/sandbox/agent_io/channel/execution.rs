use std::sync::Arc;

use crate::bridge::{
    allocation::AllocationRecord,
    ingress::StoredInbox,
    invoke::{
        BridgeInvokeEffect, BridgeInvokeGrantId, BridgeInvokeProof, BridgeInvokeRequest,
        BridgeInvokeService,
    },
};
use crate::sandbox::agent_io::{
    AdmittedAgentCommand, AgentActor, AgentCommandEnvelope, AgentResponseStatus,
    CommandLedgerError, CommandWorkState,
};

use super::{
    ChannelAgentCommandService, ChannelCommandAdmissionError as Error, ChannelCommandHold,
    ChannelCommandReservation, ChannelCommandSidecar,
};

/// Historical inputs loaded together by the Channel adapter. They cannot grant
/// authority; the service derives and checks a fresh exact-command invoke proof.
#[derive(Clone, PartialEq, Eq)]
pub struct ChannelCommandExecutionSource {
    pub inbox: StoredInbox,
    pub allocation: AllocationRecord,
    pub sidecar: ChannelCommandSidecar,
}

#[derive(Clone)]
pub struct ChannelExecutionAuthorization {
    invokes: BridgeInvokeService,
    fence: Arc<ChannelExecutionFence>,
}

/// Opaque authority for one Channel message. The common conversation store
/// consumes it alongside the same command lease, input, log and parent mutation.
pub struct ChannelExecutionFence {
    source: ChannelCommandReservation,
    envelope: AgentCommandEnvelope,
    sidecar: ChannelCommandSidecar,
}

impl std::fmt::Debug for ChannelExecutionFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelExecutionFence([REDACTED])")
    }
}

impl ChannelAgentCommandService {
    pub async fn authorize_execution(
        &self,
        invokes: &BridgeInvokeService,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelExecutionAuthorization, Error> {
        command.validate()?;
        let AgentActor::Channel { grant_id, .. } = &command.receipt.envelope.actor else {
            return Err(Error::SourceMismatch);
        };
        let source = self.store.channel_execution_source(command).await?;
        let proof = invokes
            .authorize(BridgeInvokeRequest {
                grant_id: BridgeInvokeGrantId(grant_id.clone()),
                pin: source.inbox.pin.clone(),
                target: source.allocation.placement.target().clone(),
                effect: BridgeInvokeEffect::ExecuteCommand {
                    command_id: command.receipt.envelope.command_id.clone(),
                },
            })
            .await?;
        let reservation =
            ChannelCommandReservation::prepare_source(proof, source.inbox, source.allocation)?;
        reservation.require_result(command)?;
        let mut expected = reservation.sidecar()?;
        expected.execution_hold = source.sidecar.execution_hold;
        if expected != source.sidecar {
            return Err(Error::SourceMismatch);
        }
        let authority = ChannelExecutionAuthorization {
            invokes: invokes.clone(),
            fence: Arc::new(ChannelExecutionFence {
                source: reservation,
                envelope: command.receipt.envelope.clone(),
                sidecar: source.sidecar,
            }),
        };
        authority
            .fence
            .require_envelope(&command.receipt.envelope)?;
        Ok(authority)
    }

    pub async fn release_execution_hold(
        &self,
        authority: &ChannelExecutionAuthorization,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        authority.fence.require_held(command)?;
        if let Some(gates) = &self.feature_gates {
            let source = &authority.fence.source;
            let config = &source
                .proof()
                .bridge()
                .pinned(&source.inbox().pin)
                .map_err(|_| Error::SourceMismatch)?
                .config;
            gates
                .check_feature(
                    crate::bridge::BridgeFeature::Execution,
                    &config.connector.platform,
                )
                .map_err(|_| Error::Unavailable)?;
        }
        tokio::time::timeout_at(
            authority.fence.proof().deadline(),
            self.store
                .release_channel_command(authority.fence(), command),
        )
        .await
        .map_err(|_| Error::Unavailable)??;
        Ok(())
    }
}

impl ChannelExecutionAuthorization {
    pub fn fence(&self) -> Arc<ChannelExecutionFence> {
        self.fence.clone()
    }

    /// Refresh live state immediately before the ordered runtime write while
    /// retaining the original grant, custody, Bridge snapshot and deadline.
    pub async fn current_proof(&self) -> Result<BridgeInvokeProof, Error> {
        self.fence.require_current()?;
        let original = self.fence.proof();
        let proof = tokio::time::timeout_at(
            original.deadline(),
            self.invokes.authorize(BridgeInvokeRequest {
                grant_id: original.grant().id.clone(),
                pin: original.grant().pin.clone(),
                target: original.grant().target.ids.clone(),
                effect: original.effect().clone(),
            }),
        )
        .await
        .map_err(|_| Error::Unavailable)??;
        if proof.grant() != original.grant()
            || proof.bridge() != original.bridge()
            || proof.custody_pin() != original.custody_pin()
        {
            return Err(Error::SourceMismatch);
        }
        self.fence.require_current()?;
        Ok(proof)
    }
}

impl ChannelExecutionFence {
    pub fn proof(&self) -> &BridgeInvokeProof {
        self.source.proof()
    }
    pub fn source(&self) -> &ChannelCommandReservation {
        &self.source
    }
    pub fn sidecar(&self) -> &ChannelCommandSidecar {
        &self.sidecar
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.source.require_current()?;
        if self.proof().effect()
            != &(BridgeInvokeEffect::ExecuteCommand {
                command_id: self.envelope.command_id.clone(),
            })
        {
            return Err(Error::SourceMismatch);
        }
        Ok(())
    }
    pub fn require_envelope(&self, envelope: &AgentCommandEnvelope) -> Result<(), Error> {
        self.require_current()?;
        if envelope != &self.envelope {
            return Err(Error::SourceMismatch);
        }
        Ok(())
    }
    pub fn require_dispatch(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.require_envelope(&command.receipt.envelope)?;
        self.source.require_result(command)?;
        if self.sidecar.execution_hold != ChannelCommandHold::Released {
            return Err(CommandLedgerError::NotAdmissible.into());
        }
        Ok(())
    }
    pub fn require_held(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.require_envelope(&command.receipt.envelope)?;
        self.source.require_result(command)?;
        if self.sidecar.execution_hold != ChannelCommandHold::WorkerUnavailable
            || command.status != AgentResponseStatus::WaitingAction
            || command.work != CommandWorkState::default()
        {
            return Err(CommandLedgerError::Conflict.into());
        }
        Ok(())
    }
}
