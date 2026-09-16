//! Local negative evidence cannot grant execution authority.
use super::*;
use crate::bridge::{
    channel_control::ChannelControlHold,
    invoke::{BridgeInvokeGrant, BridgeInvokeGrantState},
    outbox::{ChannelControlThread, OutboxSource},
    partition::NormalizedPartitionSource,
    BridgeRecord,
};
use crate::sandbox::{
    agent_io::{AgentResponseStatus, ChannelCommandHold, CommandFailureCode, CommandWorkState},
    Agent,
};

/// Store-produced evidence from one transaction. The store must additionally
/// verify the canonical accepted callback, intent/reference copies and complete
/// consent history containing the exact original policy and receipt.
pub struct ChannelControlRefusalSnapshot {
    pub admitted: AdmittedChannelControl,
    pub source_command: AdmittedAgentCommand,
    pub source: ChannelCommandExecutionSource,
    pub bridge: BridgeRecord,
    pub grant: BridgeInvokeGrant,
    pub consent: SourceControlPolicy,
    pub agent: Agent,
}

impl ChannelControlRefusalSnapshot {
    pub fn failure(&self) -> Result<CommandFailureCode, Error> {
        self.admitted.validate()?;
        self.source_command.validate().map_err(|_| Error::Corrupt)?;
        self.agent
            .validate_conversation()
            .map_err(|_| Error::Corrupt)?;
        self.consent.validate().map_err(|_| Error::Corrupt)?;
        self.grant.validate().map_err(|_| Error::Corrupt)?;
        let held = &self.admitted;
        let intent = &held.sidecar.intent;
        let binding = intent.reference.binding();
        let original = &binding.source;
        binding
            .require_allocation(&self.source.allocation)
            .map_err(|_| Error::Conflict)?;
        self.consent
            .view
            .require_grant(&self.grant)
            .map_err(|_| Error::Corrupt)?;
        binding
            .consent
            .view
            .require_grant(&self.grant)
            .map_err(|_| Error::Corrupt)?;
        let captured = OutboxSource::capture(
            &self.bridge,
            &self.source.inbox,
            &self.source.allocation,
            &self.source_command.receipt,
            &self.source.sidecar,
        )
        .map_err(|_| Error::Corrupt)?;
        let normalized = NormalizedPartitionSource::from_stored(&self.source.inbox, &self.bridge)
            .map_err(|_| Error::Corrupt)?;
        let target = original.target();
        if held.sidecar.execution_hold != ChannelControlHold::WorkerUnavailable
            || held.command.status != AgentResponseStatus::WaitingAction
            || held.command.work != CommandWorkState::default()
            || held.command.queue_expired(&crate::now_rfc3339())
            || captured != *original
            || self.source.sidecar.execution_hold != ChannelCommandHold::Released
            || self.grant.pin != *original.pin()
            || self.grant.owner != *original.owner()
            || self.grant.target.ids != *target
            || self.grant.config != *original.config()
            || normalized.sender_id() != binding.sender_id
            || ChannelControlThread::capture(normalized.thread()) != binding.thread
            || normalized.provider_message_id() != Some(binding.provider_message_id.as_str())
            || self.agent.id != target.agent_id
            || self.agent.sandbox_id != target.sandbox_id
            || self.agent.workspace_id != target.workspace_id
            || self.agent.worktree_id != target.worktree_id
            || self.agent.created_at != binding.agent_created_at
            || self.agent.creation_fingerprint != binding.creation_fingerprint
            || self.agent.revision < intent.expected_agent_revision
            || self.source_command.revision < intent.observed_source_command_revision
            || self
                .source_command
                .work
                .dispatch
                .as_ref()
                .is_none_or(|dispatch| {
                    dispatch.turn_sequence != binding.turn_sequence
                        || dispatch.runtime_generation != intent.runtime_generation
                })
        {
            return Err(Error::Conflict);
        }
        if matches!(self.grant.state, BridgeInvokeGrantState::Revoked { .. })
            || self.consent.view.revision > binding.consent.view.revision
        {
            return Ok(CommandFailureCode::AuthorityRevoked);
        }
        if self.consent != binding.consent {
            return Err(Error::Corrupt);
        }
        if self.source_command.status.is_terminal()
            || self
                .agent
                .conversation
                .active_turn
                .is_some_and(|turn| turn > binding.turn_sequence)
            || self
                .agent
                .conversation
                .runtime_generation
                .is_some_and(|generation| generation > intent.runtime_generation)
        {
            return Ok(CommandFailureCode::TargetTurnChanged);
        }
        // Missing identity, a regressed sequence, or a different conversation
        // under the same generation is not conclusive proof of replacement.
        Err(Error::Conflict)
    }
}
