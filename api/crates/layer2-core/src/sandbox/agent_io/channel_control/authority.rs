use super::*;
use crate::bridge::{
    channel_control::ChannelControlHold,
    invoke::{BridgeInvokeEffect, BridgeInvokeRequest},
};
use crate::sandbox::agent_io::{AgentCommand, AgentResponseStatus, CommandClaim, CommandWorkState};

#[derive(Clone)]
pub struct ChannelControlExecutionAuthorization {
    store: Arc<dyn ChannelControlExecutionStore>,
    invokes: BridgeInvokeService,
    fence: Arc<ChannelControlExecutionFence>,
}

pub struct ChannelControlExecutionFence {
    proof: BridgeInvokeProof,
    snapshot: ChannelControlExecutionSnapshot,
    claim: Option<CommandClaim>,
    deadline: tokio::time::Instant,
}
impl std::fmt::Debug for ChannelControlExecutionFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelControlExecutionFence([REDACTED])")
    }
}
impl ChannelControlExecutionAuthorization {
    pub(super) async fn observe(
        store: Arc<dyn ChannelControlExecutionStore>,
        invokes: BridgeInvokeService,
        command: &AdmittedAgentCommand,
        deadline: tokio::time::Instant,
        original: Option<&ChannelControlExecutionFence>,
    ) -> Result<Self, Error> {
        let work = async {
            command.validate().map_err(|_| Error::Corrupt)?;
            if !matches!(
                command.receipt.envelope.command,
                AgentCommand::Steer { .. } | AgentCommand::Interrupt
            ) {
                return Err(Error::Invalid);
            }
            let snapshot = store.read_control_execution(command, deadline).await?;
            // The durable dispatch binding and lease renewal may advance this
            // command after observation. Retain the original identity/claim,
            // while requiring the freshly read revision never to go backward.
            let current = &snapshot.admitted.command;
            if current.revision < command.revision
                || current.receipt != command.receipt
                || current.key_sha256 != command.key_sha256
                || current.target_turn != command.target_turn
                || current.work.lease.as_ref().map(|lease| &lease.claim)
                    != command.work.lease.as_ref().map(|lease| &lease.claim)
            {
                return Err(Error::Conflict);
            }
            let intent = &snapshot.admitted.sidecar.intent;
            let binding = intent.reference.binding();
            let proof = invokes
                .authorize(BridgeInvokeRequest {
                    grant_id: binding.consent.view.grant_id.clone(),
                    pin: binding.source.pin().clone(),
                    target: binding.source.target().clone(),
                    effect: BridgeInvokeEffect::ExecuteCommand {
                        command_id: binding.source.command_id().clone(),
                    },
                })
                .await
                .map_err(invoke_error)?;
            validation::require_snapshot(&proof, &snapshot)?;
            let fence = ChannelControlExecutionFence {
                deadline: deadline.min(proof.deadline()),
                claim: command.work.lease.as_ref().map(|lease| lease.claim.clone()),
                proof,
                snapshot,
            };
            fence.require_command(&fence.snapshot.admitted.command)?;
            if let Some(original) = original {
                original.require_current()?;
                if fence.proof.grant() != original.proof.grant()
                    || fence.proof.bridge() != original.proof.bridge()
                    || fence.proof.custody_pin() != original.proof.custody_pin()
                    || fence.snapshot.admitted.sidecar.intent
                        != original.snapshot.admitted.sidecar.intent
                    || fence.snapshot.admitted.sidecar.execution_hold
                        != original.snapshot.admitted.sidecar.execution_hold
                    || fence.claim != original.claim
                {
                    return Err(Error::Conflict);
                }
            }
            fence.require_current()?;
            Ok(Self {
                store: store.clone(),
                invokes: invokes.clone(),
                fence: Arc::new(fence),
            })
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| Error::Unavailable)?
    }
    pub fn fence(&self) -> Arc<ChannelControlExecutionFence> {
        self.fence.clone()
    }
    /// Current resource/source progress may change; original intent, claim,
    /// consent, runtime and deadline never change during this attempt.
    pub async fn refreshed(&self) -> Result<Self, Error> {
        self.fence.require_current()?;
        Self::observe(
            self.store.clone(),
            self.invokes.clone(),
            &self.fence.snapshot.admitted.command,
            self.fence.deadline,
            Some(&self.fence),
        )
        .await
    }
}
impl ChannelControlExecutionFence {
    pub fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub fn proof(&self) -> &BridgeInvokeProof {
        &self.proof
    }
    pub fn snapshot(&self) -> &ChannelControlExecutionSnapshot {
        &self.snapshot
    }
    pub fn claim(&self) -> Option<&CommandClaim> {
        self.claim.as_ref()
    }
    pub fn require_current(&self) -> Result<(), Error> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(Error::Unavailable);
        }
        self.proof.require_current().map_err(invoke_error)
    }
    pub fn require_snapshot(&self, current: &ChannelControlExecutionSnapshot) -> Result<(), Error> {
        self.require_current()?;
        self.require_command(&current.admitted.command)?;
        if current.admitted.sidecar != self.snapshot.admitted.sidecar {
            return Err(Error::Conflict);
        }
        validation::require_snapshot(&self.proof, current)
    }
    pub fn require_command(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.require_current()?;
        command.validate().map_err(|_| Error::Corrupt)?;
        let original = &self.snapshot.admitted.command;
        if command.revision < original.revision
            || command.receipt != original.receipt
            || command.key_sha256 != original.key_sha256
            || command.target_turn != original.target_turn
            || command.work.lease.as_ref().map(|lease| &lease.claim) != self.claim.as_ref()
            || command.status.is_terminal()
        {
            return Err(Error::Conflict);
        }
        if let Some(claim) = &self.claim {
            command
                .require_claim(claim, &crate::now_rfc3339())
                .map_err(|_| Error::Conflict)?;
        }
        Ok(())
    }
    pub fn require_held(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.require_command(command)?;
        if self.snapshot.admitted.sidecar.execution_hold != ChannelControlHold::WorkerUnavailable
            || command.status != AgentResponseStatus::WaitingAction
            || command.work != CommandWorkState::default()
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    pub fn require_dispatch(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.require_command(command)?;
        if self.snapshot.admitted.sidecar.execution_hold != ChannelControlHold::Released
            || !matches!(
                command.status,
                AgentResponseStatus::Queued | AgentResponseStatus::InProgress
            )
            || command.queue_expired(&crate::now_rfc3339())
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}
