use crate::bridge::{
    allocation::AllocationRecord,
    ingress::StoredInbox,
    invoke::{BridgeInvokeEffect, BridgeInvokeProof, SourceControlPolicy, SourceControlReceipt},
    outbox::{
        ChannelControlReferenceRecord, ChannelControlThread, DeliveryState, OutboxItem,
        OutboxSource,
    },
    partition::ChannelConversationKind,
};
use crate::sandbox::agent_io::{
    AdmittedAgentCommand, AgentActor, AgentResponseStatus, ChannelCommandHold,
    ChannelCommandSidecar,
};
use crate::RuntimeCapability;

use super::{
    ChannelControlCallback, ChannelControlCommand, ChannelControlError as Error,
    ChannelControlIntent, ChannelControlWrite,
};

/// Trusted storage observations. The adapter must reread and write-fence these
/// exact rows, complete consent history and current custody in one transaction.
pub struct ChannelControlObservation<'a> {
    pub reference: &'a ChannelControlReferenceRecord,
    pub original_inbox: &'a StoredInbox,
    pub source_command: &'a AdmittedAgentCommand,
    pub source_sidecar: &'a ChannelCommandSidecar,
    pub allocation: &'a AllocationRecord,
    pub consent: &'a SourceControlPolicy,
    pub consent_receipt: &'a SourceControlReceipt,
    pub replied_item: Option<&'a OutboxItem>,
}

/// Independent opaque control admission proof. The original Message invoke
/// proof supplies current source/resource facts only; explicit exact-source
/// consent and this signed human callback establish the additional permission.
/// Neither proof is accepted by the ordinary Message execution path for controls.
pub struct ChannelControlProof {
    write: ChannelControlWrite,
    source_invoke: BridgeInvokeProof,
    intent: ChannelControlIntent,
    source_command: AdmittedAgentCommand,
    source_sidecar: ChannelCommandSidecar,
    original_inbox: StoredInbox,
    allocation: AllocationRecord,
    replied_item: Option<OutboxItem>,
}

impl ChannelControlProof {
    pub fn from_current(
        write: ChannelControlWrite,
        source_invoke: BridgeInvokeProof,
        observed: ChannelControlObservation<'_>,
    ) -> Result<Self, Error> {
        write.require_current()?;
        source_invoke
            .require_current()
            .map_err(|_| Error::AuthorityUnavailable)?;
        validate_source(&write, &source_invoke, &observed)?;
        let agent = source_invoke
            .resources()
            .lineage
            .agent
            .as_ref()
            .ok_or(Error::SourceUnavailable)?;
        let callback = ChannelControlCallback::capture(&write)?;
        let intent = ChannelControlIntent {
            schema_version: 1,
            id: callback.intent_id()?,
            callback,
            request: write.request().clone(),
            reference: observed.reference.clone(),
            expected_agent_revision: agent.revision,
            observed_source_command_revision: observed.source_command.revision,
            runtime_generation: agent
                .conversation
                .runtime_generation
                .ok_or(Error::SourceUnavailable)?,
            runtime_conversation_id: agent
                .conversation
                .conversation_id
                .clone()
                .ok_or(Error::SourceUnavailable)?,
            captured_at: write.inbox().record().accepted_at,
        };
        intent.validate()?;
        Ok(Self {
            write,
            source_invoke,
            intent,
            source_command: observed.source_command.clone(),
            source_sidecar: observed.source_sidecar.clone(),
            original_inbox: observed.original_inbox.clone(),
            allocation: observed.allocation.clone(),
            replied_item: observed.replied_item.cloned(),
        })
    }

    pub fn write(&self) -> &ChannelControlWrite {
        &self.write
    }
    pub fn source_invoke(&self) -> &BridgeInvokeProof {
        &self.source_invoke
    }
    pub fn intent(&self) -> &ChannelControlIntent {
        &self.intent
    }
    pub fn source_command(&self) -> &AdmittedAgentCommand {
        &self.source_command
    }
    pub fn source_sidecar(&self) -> &ChannelCommandSidecar {
        &self.source_sidecar
    }
    pub fn original_inbox(&self) -> &StoredInbox {
        &self.original_inbox
    }
    pub fn allocation(&self) -> &AllocationRecord {
        &self.allocation
    }
    pub fn replied_item(&self) -> Option<&OutboxItem> {
        self.replied_item.as_ref()
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.write.require_current()?;
        self.source_invoke
            .require_current()
            .map_err(|_| Error::AuthorityUnavailable)?;
        self.intent.validate()
    }

    /// Recovery cannot refresh the source, callback, captured revision or time.
    /// A provider alias recovers the original receipt in storage before this
    /// path; it never constructs a new proof for an already retained intent.
    pub fn require_intent(&self, retained: &ChannelControlIntent) -> Result<(), Error> {
        self.require_current()?;
        retained.validate()?;
        if retained != &self.intent {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

fn validate_source(
    write: &ChannelControlWrite,
    invoke: &BridgeInvokeProof,
    observed: &ChannelControlObservation<'_>,
) -> Result<(), Error> {
    observed
        .source_command
        .validate()
        .map_err(|_| Error::Corrupt)?;
    observed.allocation.validate().map_err(|_| Error::Corrupt)?;
    observed.reference.validate().map_err(|_| Error::Corrupt)?;
    let binding = observed.reference.binding();
    binding
        .require_allocation(observed.allocation)
        .map_err(|_| Error::SourceUnavailable)?;
    let source = &binding.source;
    let grant = invoke.grant();
    let basis = grant.channel.as_ref().ok_or(Error::SourceUnavailable)?;
    let resources = invoke.resources();
    let agent = resources
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::SourceUnavailable)?;
    let inbox = write.inbox().record();
    let normalized = write.source();
    observed
        .consent_receipt
        .require_policy(observed.consent)
        .map_err(|_| Error::AuthorityUnavailable)?;
    observed
        .consent
        .view
        .require_grant(grant)
        .map_err(|_| Error::AuthorityUnavailable)?;
    let captured = OutboxSource::capture(
        invoke.bridge(),
        observed.original_inbox,
        &basis.allocation,
        &observed.source_command.receipt,
        observed.source_sidecar,
    )
    .map_err(|_| Error::SourceUnavailable)?;
    let actor_matches = matches!(&observed.source_command.receipt.envelope.actor,
        AgentActor::Channel { grant_id, .. } if grant_id == grant.id.as_str());
    if write.request().reference != observed.reference.reference
        || invoke.effect()
            != &(BridgeInvokeEffect::ExecuteCommand {
                command_id: source.command_id().clone(),
            })
        || write.inbox().bridge() != invoke.bridge()
        || inbox.pin != *source.pin()
        || inbox.owner != *source.owner()
        || normalized.bot_id() != source.config().connector.bot_id
        || normalized.issuer() != source.config().connector.issuer
        || normalized.platform() != &source.config().connector.platform
        || normalized.chat_id() != source.chat_id()
        || normalized.sender_id() != binding.sender_id
        || ChannelControlThread::capture(normalized.thread()) != binding.thread
        || captured != *source
        || !actor_matches
        || grant.pin != *source.pin()
        || grant.owner != *source.owner()
        || grant.target.ids != *source.target()
        || grant.config != *source.config()
        || !observed.consent.view.enabled
        || observed.consent != &binding.consent
        || observed.consent_receipt != &binding.consent_receipt
        || basis.allocation.identity != observed.allocation.identity
        || basis.allocation.created_at_ms != observed.allocation.created_at_ms
        || basis.allocation.placement != observed.allocation.placement
        || agent.created_at != binding.agent_created_at
        || agent.creation_fingerprint != binding.creation_fingerprint
        || agent.conversation.active_turn != Some(binding.turn_sequence)
        || !matches!(
            observed.source_command.status,
            AgentResponseStatus::InProgress | AgentResponseStatus::WaitingAction
        )
        || observed.source_sidecar.execution_hold != ChannelCommandHold::Released
        || observed
            .source_command
            .work
            .dispatch
            .as_ref()
            .is_none_or(|dispatch| {
                dispatch.turn_sequence != binding.turn_sequence
                    || Some(dispatch.runtime_generation) != agent.conversation.runtime_generation
            })
    {
        return Err(Error::SourceUnavailable);
    }
    basis
        .creation
        .validate_against(agent, &resources.lineage.sandbox)
        .map_err(|_| Error::SourceUnavailable)?;
    let capability = match write.request().command {
        ChannelControlCommand::Cancel => RuntimeCapability::Interrupt,
        ChannelControlCommand::Steer { .. } => RuntimeCapability::Steer,
    };
    if !agent.capabilities.supports(capability) {
        return Err(Error::CapabilityUnsupported);
    }
    if normalized.conversation_kind() != ChannelConversationKind::Private {
        require_reply(write, binding, observed.replied_item)?;
    }
    Ok(())
}

fn require_reply(
    write: &ChannelControlWrite,
    binding: &crate::bridge::outbox::ChannelControlReferenceBinding,
    item: Option<&OutboxItem>,
) -> Result<(), Error> {
    let inbox = write.inbox().record();
    let evidence = inbox
        .capabilities
        .as_ref()
        .and_then(|facts| facts.confirmed_reply_receipt())
        .ok_or(Error::SourceUnavailable)?;
    let item = item.ok_or(Error::SourceUnavailable)?;
    item.validate().map_err(|_| Error::Corrupt)?;
    let DeliveryState::Sent { receipt } = &item.state else {
        return Err(Error::SourceUnavailable);
    };
    if item.source != binding.source
        || item.turn_sequence != binding.turn_sequence
        || item.id != evidence.item_id
        || item.revision != evidence.revision
        || super::hash(item)? != evidence.record_sha256
        || receipt.platform_message_id.is_none()
        || receipt.platform_message_id != inbox.envelope.reply_to_platform_message_id
    {
        return Err(Error::SourceUnavailable);
    }
    Ok(())
}
