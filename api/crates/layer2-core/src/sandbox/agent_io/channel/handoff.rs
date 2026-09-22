//! A held idle input can carry its just-validated authority through the same
//! worker attempt. No cloning, persistence, shared cache or deadline renewal.
use std::sync::Arc;

use crate::bridge::invoke::{BridgeInvokeEffect, BridgeInvokeRequest};
use crate::sandbox::agent_io::{AdmittedAgentCommand, AgentCommand, CommandLedgerError};
use crate::sandbox::{AgentStatus, SandboxStatus};

use super::{
    ChannelAgentCommandService, ChannelCommandAdmissionError as Error, ChannelCommandHold,
    ChannelExecutionAuthorization, ChannelExecutionFence,
};

pub struct ChannelExecutionHandoff {
    authority: ChannelExecutionAuthorization,
    held: AdmittedAgentCommand,
}

impl ChannelAgentCommandService {
    /// Construct only after the corresponding hold release completed. The next
    /// claim must still match that exact release; this alone authorizes no write.
    pub fn idle_execution_handoff(
        &self,
        authority: ChannelExecutionAuthorization,
        held: &AdmittedAgentCommand,
    ) -> Option<ChannelExecutionHandoff> {
        authority.fence.require_held(held).ok()?;
        let lineage = &authority.fence.proof().resources().lineage;
        let agent = lineage.agent.as_ref()?;
        if authority.fence.supersession.is_some()
            || agent.status != AgentStatus::Ready
            || agent.conversation.active_turn.is_some()
            || agent.requested_stop_reason.is_some()
            || !matches!(
                lineage.sandbox.status,
                SandboxStatus::Active | SandboxStatus::CredentialRotating
            )
            || !matches!(held.receipt.envelope.command, AgentCommand::Message { .. })
        {
            return None;
        }
        Some(ChannelExecutionHandoff {
            authority,
            held: held.clone(),
        })
    }

    /// Consume once for the immediate release→claim transition. All resource
    /// observations must still match, and dispatch keeps its normal transaction
    /// fences and fresh ordered-writer authorization. Drift falls back upstream.
    pub async fn resume_idle_execution(
        &self,
        handoff: ChannelExecutionHandoff,
        command: &AdmittedAgentCommand,
    ) -> Result<ChannelExecutionAuthorization, Error> {
        let ChannelExecutionHandoff { authority, held } = handoff;
        authority.fence.require_held(&held)?;
        command.validate()?;
        let claim = &command
            .work
            .lease
            .as_ref()
            .ok_or(CommandLedgerError::Conflict)?
            .claim;
        command.require_claim(claim, &crate::now_rfc3339())?;
        if command.receipt != held.receipt
            || command.key_sha256 != held.key_sha256
            || command.target_turn != held.target_turn
            || Some(command.revision) != held.revision.checked_add(2)
            || command.work.dispatch.is_some()
        {
            return Err(Error::SourceMismatch);
        }
        let source = self.store.channel_execution_source(command).await?;
        authority
            .fence
            .source
            .require_records(&source.inbox, &source.allocation)?;
        let mut sidecar = authority.fence.sidecar.clone();
        sidecar.execution_hold = ChannelCommandHold::Released;
        if sidecar != source.sidecar {
            return Err(Error::SourceMismatch);
        }
        let original = authority.fence.proof();
        let current = authority
            .invokes
            .observe_execution_readiness(BridgeInvokeRequest {
                grant_id: original.grant().id.clone(),
                pin: original.grant().pin.clone(),
                target: original.grant().target.ids.clone(),
                effect: BridgeInvokeEffect::ExecuteCommand {
                    command_id: command.receipt.envelope.command_id.clone(),
                },
            })
            .await?;
        if &current != original.resources() {
            return Err(Error::SourceMismatch);
        }
        let resumed = ChannelExecutionAuthorization {
            invokes: authority.invokes,
            fence: Arc::new(ChannelExecutionFence {
                source: authority.fence.source.clone(),
                envelope: authority.fence.envelope.clone(),
                sidecar,
                supersession: None,
            }),
        };
        resumed.fence.require_dispatch(command)?;
        Ok(resumed)
    }
}
