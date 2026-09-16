use super::*;
use crate::bridge::{
    invoke::BridgeInvokeEffect,
    outbox::{ChannelControlThread, OutboxSource},
    partition::NormalizedPartitionSource,
};
use crate::sandbox::agent_io::{AgentActor, AgentResponseStatus, ChannelCommandHold};
use crate::sandbox::{AgentStatus, SandboxStatus};
use crate::RuntimeCapability;

pub(super) fn require_snapshot(
    proof: &BridgeInvokeProof,
    snapshot: &ChannelControlExecutionSnapshot,
) -> Result<(), Error> {
    require_source(proof, snapshot)?;
    if let Some(lease) = &snapshot.admitted.command.work.lease {
        let lane = snapshot.lane.as_ref().ok_or(Error::Conflict)?;
        lane.require_claim(&lease.claim, &crate::now_rfc3339())
            .map_err(|_| Error::Conflict)?;
        snapshot
            .admitted
            .command
            .require_claim(&lease.claim, &crate::now_rfc3339())
            .map_err(|_| Error::Conflict)?;
        if lane.lease.as_ref() != Some(lease)
            || snapshot.admitted.command.receipt.agent_order > lane.last_order
        {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

pub(super) fn require_ready_snapshot(
    proof: &BridgeInvokeProof,
    snapshot: &ChannelControlExecutionSnapshot,
) -> Result<(), Error> {
    require_source(proof, snapshot)?;
    let command = &snapshot.admitted.command;
    let now = crate::now_rfc3339();
    if snapshot.admitted.sidecar.execution_hold
        != crate::bridge::channel_control::ChannelControlHold::Released
        || !command.awaiting_claim(&now)
        || command.queue_expired(&now)
    {
        return Err(Error::Conflict);
    }
    if let Some(expired) = &command.work.lease {
        let lane = snapshot.lane.as_ref().ok_or(Error::Conflict)?;
        // Another worker may have used and released this lane since the old
        // claim expired. Only the atomic claim transaction selects a new claim.
        if lane.lease.as_ref().is_some_and(|lease| lease.live_at(&now))
            || lane.next_fence <= expired.claim.fence
            || command.receipt.agent_order > lane.last_order
        {
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

fn require_source(
    proof: &BridgeInvokeProof,
    snapshot: &ChannelControlExecutionSnapshot,
) -> Result<(), Error> {
    snapshot.admitted.validate()?;
    let intent = &snapshot.admitted.sidecar.intent;
    let binding = intent.reference.binding();
    let source = &binding.source;
    let grant = proof.grant();
    let basis = grant.channel.as_ref().ok_or(Error::SourceUnavailable)?;
    let agent = proof
        .resources()
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::SourceUnavailable)?;
    let parent = &proof.resources().lineage.sandbox;
    snapshot
        .consent_receipt
        .require_policy(&snapshot.consent)
        .map_err(|_| Error::AuthorityUnavailable)?;
    snapshot
        .consent
        .view
        .require_grant(grant)
        .map_err(|_| Error::AuthorityUnavailable)?;
    let original = &snapshot.source;
    binding
        .require_allocation(&original.allocation)
        .map_err(|_| Error::SourceUnavailable)?;
    let captured = OutboxSource::capture(
        proof.bridge(),
        &original.inbox,
        &basis.allocation,
        &snapshot.source_command.receipt,
        &original.sidecar,
    )
    .map_err(|_| Error::SourceUnavailable)?;
    let normalized = NormalizedPartitionSource::from_stored(&original.inbox, proof.bridge())
        .map_err(|_| Error::SourceUnavailable)?;
    let capability = match intent.request.command {
        crate::bridge::channel_control::ChannelControlCommand::Cancel => {
            RuntimeCapability::Interrupt
        }
        crate::bridge::channel_control::ChannelControlCommand::Steer { .. } => {
            RuntimeCapability::Steer
        }
    };
    if !agent.capabilities.supports(capability)
        || !agent.capabilities.supports(RuntimeCapability::Conversation)
    {
        return Err(Error::CapabilityUnsupported);
    }
    if snapshot.consent != binding.consent
        || snapshot.consent_receipt != binding.consent_receipt
        || !snapshot.consent.view.enabled
        || grant.pin != *source.pin()
        || grant.owner != *source.owner()
        || grant.target.ids != *source.target()
        || grant.config != *source.config()
        || captured != *source
        || proof
            .bridge()
            .admission_pin()
            .map_err(|_| Error::AuthorityUnavailable)?
            != *source.pin()
        || proof.effect()
            != &(BridgeInvokeEffect::ExecuteCommand {
                command_id: source.command_id().clone(),
            })
        || !matches!(&snapshot.source_command.receipt.envelope.actor, AgentActor::Channel { grant_id, .. } if grant_id == grant.id.as_str())
        || normalized.sender_id() != binding.sender_id
        || ChannelControlThread::capture(normalized.thread()) != binding.thread
        || normalized.provider_message_id() != Some(binding.provider_message_id.as_str())
        || basis.allocation.identity != original.allocation.identity
        || basis.allocation.placement != original.allocation.placement
        || basis.allocation.created_at_ms != original.allocation.created_at_ms
        || original.sidecar.execution_hold != ChannelCommandHold::Released
        || agent.revision < intent.expected_agent_revision
        || agent.created_at != binding.agent_created_at
        || agent.creation_fingerprint != binding.creation_fingerprint
        || agent.conversation.active_turn != Some(binding.turn_sequence)
        || agent.conversation.runtime_generation != Some(intent.runtime_generation)
        || agent.conversation.conversation_id.as_ref() != Some(&intent.runtime_conversation_id)
        || agent.requested_stop_reason.is_some()
        || !matches!(
            agent.status,
            AgentStatus::Working | AgentStatus::WaitingApproval
        )
        || !matches!(
            parent.status,
            SandboxStatus::Active | SandboxStatus::CredentialRotating
        )
        || parent.lifecycle.checkpoint_capture.is_some()
        || snapshot.source_command.revision < intent.observed_source_command_revision
        || !matches!(
            snapshot.source_command.status,
            AgentResponseStatus::InProgress | AgentResponseStatus::WaitingAction
        )
        || snapshot
            .source_command
            .work
            .dispatch
            .as_ref()
            .is_none_or(|dispatch| {
                dispatch.turn_sequence != binding.turn_sequence
                    || dispatch.runtime_generation != intent.runtime_generation
            })
    {
        return Err(Error::SourceUnavailable);
    }
    basis
        .creation
        .validate_against(agent, parent)
        .map_err(|_| Error::SourceUnavailable)?;
    Ok(())
}
