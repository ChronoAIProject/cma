use super::*;
use crate::bridge::{
    allocation::AllocationRecord,
    ingress::{InboxAdmission, StoredInbox},
    partition::{NormalizedPartitionSource, PartitionAdmissionDecision, PartitionSourcePin},
    BridgeRecord,
};
use crate::sandbox::{Agent, AgentStatus};

pub(super) fn allocation(
    manager: &BridgeManagementLease,
    record: &AllocationRecord,
) -> Result<(), PartitionStatusError> {
    manager.require_current()?;
    record
        .validate()
        .map_err(|_| PartitionStatusError::Corrupt)?;
    if record.identity.bridge_id != manager.record().id
        || record.owner != manager.record().owner
        || manager.record().pinned(&record.pin)?.config != record.config
    {
        return Err(PartitionStatusError::Corrupt);
    }
    Ok(())
}

fn source(
    bridge: &BridgeRecord,
    allocation: &AllocationRecord,
    inbox: &StoredInbox,
) -> Result<(), PartitionStatusError> {
    if inbox.owner != allocation.owner
        || inbox.pin.bridge_id != allocation.identity.bridge_id
        || inbox.admission != InboxAdmission::Accepted
    {
        return Err(PartitionStatusError::Corrupt);
    }
    let normalized = NormalizedPartitionSource::from_stored(inbox, bridge)
        .map_err(|_| PartitionStatusError::Corrupt)?;
    let config = &bridge.pinned(&inbox.pin)?.config;
    match normalized
        .admit(config)
        .map_err(|_| PartitionStatusError::Corrupt)?
    {
        PartitionAdmissionDecision::Admitted { key, .. } if key == allocation.identity.key => {
            Ok(())
        }
        _ => Err(PartitionStatusError::Corrupt),
    }
}

pub(super) fn first_source(
    bridge: &BridgeRecord,
    record: &AllocationRecord,
    inbox: &StoredInbox,
) -> Result<PartitionSourceView, PartitionStatusError> {
    source(bridge, record, inbox)?;
    if PartitionSourcePin::for_inbox(inbox).map_err(|_| PartitionStatusError::Corrupt)?
        != record.first_source
    {
        return Err(PartitionStatusError::Corrupt);
    }
    let sender_display_name = inbox
        .envelope
        .sender_display_name
        .as_deref()
        .map(|name| {
            name.chars()
                .filter(|c| !c.is_control())
                .take(256)
                .collect::<String>()
        })
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty());
    Ok(PartitionSourceView::Available {
        platform: record.config.connector.platform.clone(),
        chat_id: inbox.envelope.chat_id.clone(),
        sender_id: inbox.envelope.sender_id.clone(),
        sender_display_name,
        conversation_type: NormalizedPartitionSource::from_stored(inbox, bridge)
            .map_err(|_| PartitionStatusError::Corrupt)?
            .conversation_kind(),
    })
}

pub(super) fn command(
    bridge: &BridgeRecord,
    allocation: &AllocationRecord,
    evidence: &PartitionCommandEvidence,
    agent: &Agent,
) -> Result<Option<PartitionCommandView>, PartitionStatusError> {
    let record = &evidence.command;
    record
        .validate()
        .map_err(|_| PartitionStatusError::Corrupt)?;
    let envelope = &record.receipt.envelope;
    let sidecar = &evidence.sidecar;
    if sidecar.schema_version != 1
        || sidecar.allocation_id != allocation.id
        || sidecar.command_id != envelope.command_id
        || record.receipt.response_id.as_ref() != Some(&sidecar.response_id)
        || sidecar.request_fingerprint != envelope.request_fingerprint
        || envelope.agent_id != allocation.placement.target().agent_id
        || envelope.agent_id != agent.id
        || envelope.owner != allocation.owner
        || envelope.source.kind != AgentSourceKind::Channel
        || envelope.source.reference != sidecar.inbox_id
        || envelope.correlation_id != sidecar.inbox_id
    {
        return Err(PartitionStatusError::Corrupt);
    }
    let AgentActor::Channel {
        bridge_id,
        platform,
        chat_id,
        sender_id,
        ..
    } = &envelope.actor
    else {
        return Err(PartitionStatusError::Corrupt);
    };
    if bridge_id != allocation.identity.bridge_id.as_str() {
        return Err(PartitionStatusError::Corrupt);
    }
    let Some(inbox) = &evidence.source else {
        return Ok(None);
    };
    source(bridge, allocation, inbox)?;
    if inbox.id != sidecar.inbox_id || inbox.attestation.body_sha256 != sidecar.body_sha256 {
        return Err(PartitionStatusError::Corrupt);
    }
    if bridge_id != inbox.pin.bridge_id.as_str()
        || platform != &inbox.envelope.platform
        || chat_id != &inbox.envelope.chat_id
        || sender_id != &inbox.envelope.sender_id
    {
        return Err(PartitionStatusError::Corrupt);
    }
    let AgentCommand::Message { metadata, .. } = &envelope.command else {
        return Err(PartitionStatusError::Corrupt);
    };
    for (key, expected) in [
        ("channel_inbox_id", inbox.id.clone()),
        ("channel_body_sha256", inbox.attestation.body_sha256.clone()),
        ("channel_message_id", inbox.message_id.clone()),
        (
            "channel_config_revision",
            inbox.pin.config_revision.to_string(),
        ),
        ("channel_allocation_id", allocation.id.clone()),
    ] {
        if metadata.get(key) != Some(&expected) {
            return Err(PartitionStatusError::Corrupt);
        }
    }
    let turn_sequence = record
        .work
        .dispatch
        .as_ref()
        .map(|dispatch| dispatch.turn_sequence);
    let waiting_for = if record.status.is_terminal() {
        PartitionCommandWait::None
    } else if sidecar.execution_hold == ChannelCommandHold::WorkerUnavailable {
        if record.status != AgentResponseStatus::WaitingAction || record.work.dispatch.is_some() {
            return Err(PartitionStatusError::Corrupt);
        }
        PartitionCommandWait::Worker
    } else if record.status == AgentResponseStatus::WaitingAction {
        if turn_sequence.is_some()
            && turn_sequence == agent.conversation.active_turn
            && agent.status == AgentStatus::WaitingApproval
        {
            PartitionCommandWait::Approval
        } else {
            PartitionCommandWait::Action
        }
    } else {
        PartitionCommandWait::None
    };
    Ok(Some(PartitionCommandView {
        command_id: envelope.command_id.clone(),
        response_id: sidecar.response_id.clone(),
        config_revision: inbox.pin.config_revision,
        agent_order: record.receipt.agent_order,
        status: record.status,
        waiting_for,
        turn_sequence,
        delivery: record
            .work
            .observation
            .as_ref()
            .map(|v| v.delivery)
            .or_else(|| record.work.dispatch.as_ref().map(|v| v.delivery)),
        admitted_at: record.receipt.admitted_at.clone(),
    }))
}
