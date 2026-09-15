use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::bridge::{
    allocation::{AllocationPlacement, AllocationRecord, AllocationState},
    ingress::{InboxAdmission, StoredInbox},
    invoke::{BridgeInvokeEffect, BridgeInvokeProof},
    partition::{NormalizedPartitionSource, PartitionAdmissionDecision},
};
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::{agent_io::*, store::SandboxMutationGuard};

use super::{ChannelCommandAdmissionError as Error, ChannelCommandReservation};

impl ChannelCommandReservation {
    pub fn prepare(
        proof: BridgeInvokeProof,
        inbox: StoredInbox,
        allocation: AllocationRecord,
    ) -> Result<Self, Error> {
        if proof.effect()
            != &(BridgeInvokeEffect::AdmitMessage {
                inbox_id: inbox.id.clone(),
            })
        {
            return Err(Error::SourceMismatch);
        }
        Self::prepare_source(proof, inbox, allocation)
    }

    pub(super) fn prepare_source(
        proof: BridgeInvokeProof,
        inbox: StoredInbox,
        allocation: AllocationRecord,
    ) -> Result<Self, Error> {
        proof.require_current()?;
        allocation.validate().map_err(|_| Error::SourceMismatch)?;
        let grant = proof.grant();
        let retained_execution =
            matches!(proof.effect(), BridgeInvokeEffect::ExecuteCommand { .. })
                && grant
                    .channel
                    .as_ref()
                    .is_some_and(|basis| basis.require_retained_allocation(&allocation).is_ok());
        if inbox.admission != InboxAdmission::Accepted
            || inbox.pin != grant.pin
            || inbox.owner != grant.owner
            || (allocation.state != AllocationState::Ready && !retained_execution)
            || allocation.owner != inbox.owner
            || match &allocation.placement {
                AllocationPlacement::ExistingAgent { target, grant_id } => {
                    allocation.pin != inbox.pin
                        || allocation.config != grant.config
                        || target != &grant.target.ids
                        || grant_id != grant.id.as_str()
                        || grant.channel.is_some()
                }
                AllocationPlacement::NewSandbox { target, .. }
                | AllocationPlacement::ReuseParent { target, .. } => {
                    target != &grant.target.ids
                        || grant.channel.as_ref().is_none_or(|basis| {
                            basis.allocation != allocation && !retained_execution
                        })
                }
            }
        {
            return Err(Error::SourceMismatch);
        }
        let source = NormalizedPartitionSource::from_stored(&inbox, proof.bridge())?;
        match source.admit(&grant.config)? {
            PartitionAdmissionDecision::Admitted { key, .. } if key == allocation.identity.key => {}
            PartitionAdmissionDecision::Admitted { .. } => return Err(Error::SourceMismatch),
            PartitionAdmissionDecision::Refused { reason } => return Err(Error::Refused(reason)),
        }
        if inbox.envelope.content_type != "text" || !inbox.envelope.attachments.is_empty() {
            return Err(crate::sandbox::AgentOperationError::ContentUnsupported.into());
        }
        let text = inbox.envelope.text.as_ref().ok_or(Error::SourceMismatch)?;
        let mut command = AgentCommand::message(text).canonical()?;
        if let AgentCommand::Message { metadata, .. } = &mut command {
            for (key, value) in [
                ("channel_inbox_id", inbox.id.clone()),
                ("channel_body_sha256", inbox.attestation.body_sha256.clone()),
                ("channel_message_id", inbox.message_id.clone()),
                (
                    "channel_config_revision",
                    inbox.pin.config_revision.to_string(),
                ),
                ("channel_allocation_id", allocation.id.clone()),
            ] {
                metadata.insert(key.into(), value);
            }
        }
        let snapshot = proof.resources();
        let agent = snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(Error::SourceMismatch)?;
        let now = SystemTime::now();
        // Message identity does not vary with grant revision, retries, workers,
        // allocation reset or runtime incarnation. A conflict cannot mint a turn.
        let command_id = AgentCommandId(format!("cmd_{}", identity("command", &inbox.id)));
        let response_id = AgentResponseId(format!("resp_{}", identity("response", &inbox.id)));
        let reservation = CommandReservation {
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id,
                agent_id: agent.id.clone(),
                owner: inbox.owner.clone(),
                actor: AgentActor::Channel {
                    bridge_id: inbox.pin.bridge_id.as_str().into(),
                    grant_id: grant.id.as_str().into(),
                    platform: inbox.envelope.platform.clone(),
                    chat_id: source.chat_id().into(),
                    sender_id: source.sender_id().into(),
                },
                source: AgentCommandSource {
                    kind: AgentSourceKind::Channel,
                    reference: inbox.id.clone(),
                },
                correlation_id: inbox.id.clone(),
                request_fingerprint: command.fingerprint()?,
                authorization: AgentAuthorizationRevision {
                    authority_id: format!("bridge_invoke:{}", grant.id.as_str()),
                    policy_revision: snapshot.fence().policy_revision.max(1),
                    grant_revision: Some(grant.revision),
                },
                command,
            },
            key_sha256: command_key_digest(&inbox.id)?,
            response_id: Some(response_id),
            admitted_at: crate::rfc3339_at(now),
            queue_deadline: crate::rfc3339_at(now + Duration::from_secs(300)),
            queue_capacity: MAX_COMMAND_QUEUE_CAPACITY.min(
                u16::try_from(grant.config.limits.max_queued_messages)
                    .unwrap_or(MAX_COMMAND_QUEUE_CAPACITY),
            ),
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&snapshot.lineage.sandbox),
            policy: snapshot.fence(),
            authority_deadline: Some(proof.deadline()),
            // Human/organization owner audit bases must never be fabricated for
            // a Channel. Its actual grant/actor/source are in the durable receipt.
            admission_audit: None,
        };
        let result = Self {
            proof,
            inbox,
            allocation,
            reservation,
            feature_gates: None,
        };
        result.require_current()?;
        Ok(result)
    }
}

fn identity(kind: &str, inbox: &str) -> String {
    let bytes = serde_json::to_vec(&("cma.channel.command.v1", kind, inbox))
        .expect("typed channel identity is serializable");
    format!("{:x}", Sha256::digest(bytes))
}
