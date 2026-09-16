use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ids::AgentCommandId;
use crate::sandbox::{agent_io::*, store::SandboxMutationGuard};

use super::{ChannelControlError as Error, ChannelControlIntent, ChannelControlProof};

/// A separate opaque admission path. Ordinary human/API and Message admission
/// continue rejecting these Channel control commands.
pub struct ChannelControlReservation {
    proof: ChannelControlProof,
    command: CommandReservation,
}

impl ChannelControlReservation {
    pub fn prepare(proof: ChannelControlProof) -> Result<Self, Error> {
        proof.require_current()?;
        let intent = proof.intent();
        let invoke = proof.source_invoke();
        let grant = invoke.grant();
        let resources = invoke.resources();
        let source = proof.write().source();
        let command = intent.request.command.command();
        let admitted_at = UNIX_EPOCH + Duration::from_secs(intent.captured_at as u64);
        let reservation = CommandReservation {
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: intent.command_id()?,
                agent_id: grant.target.ids.agent_id.clone(),
                owner: intent.callback.owner.clone(),
                actor: AgentActor::Channel {
                    bridge_id: intent.callback.pin.bridge_id.as_str().into(),
                    grant_id: grant.id.as_str().into(),
                    platform: proof.write().inbox().record().envelope.platform.clone(),
                    chat_id: source.chat_id().into(),
                    sender_id: source.sender_id().into(),
                },
                source: AgentCommandSource {
                    kind: AgentSourceKind::Channel,
                    reference: intent.id.clone(),
                },
                correlation_id: intent.callback.callback_id.clone(),
                request_fingerprint: command.fingerprint().map_err(|_| Error::Invalid)?,
                authorization: AgentAuthorizationRevision {
                    authority_id: format!("bridge_invoke:{}", grant.id.as_str()),
                    policy_revision: resources.fence().policy_revision.max(1),
                    grant_revision: Some(grant.revision),
                },
                command,
            },
            key_sha256: command_key_digest(&intent.id).map_err(|_| Error::Invalid)?,
            response_id: None,
            admitted_at: crate::rfc3339_at(admitted_at),
            queue_deadline: crate::rfc3339_at(admitted_at + Duration::from_secs(300)),
            queue_capacity: MAX_COMMAND_QUEUE_CAPACITY.min(
                u16::try_from(grant.config.limits.max_queued_messages)
                    .unwrap_or(MAX_COMMAND_QUEUE_CAPACITY),
            ),
            expected_agent_revision: intent.expected_agent_revision,
            parent: SandboxMutationGuard::capture(&resources.lineage.sandbox),
            policy: resources.fence(),
            authority_deadline: Some(invoke.deadline()),
            // This is actual Channel attribution, never a fabricated owner audit.
            admission_audit: None,
        };
        reservation.validate().map_err(|_| Error::Invalid)?;
        let result = Self {
            proof,
            command: reservation,
        };
        result.require_current()?;
        Ok(result)
    }

    pub fn proof(&self) -> &ChannelControlProof {
        &self.proof
    }
    pub fn command(&self) -> &CommandReservation {
        &self.command
    }
    pub fn require_current(&self) -> Result<(), Error> {
        self.proof.require_current()?;
        self.command.validate().map_err(|_| Error::Conflict)
    }
    pub fn sidecar(&self) -> Result<ChannelControlSidecar, Error> {
        self.require_current()?;
        Ok(ChannelControlSidecar {
            schema_version: 1,
            command_id: self.command.envelope.command_id.clone(),
            intent: self.proof.intent().clone(),
            intent_sha256: self.proof.intent().fingerprint()?,
            execution_hold: ChannelControlHold::WorkerUnavailable,
        })
    }
    pub fn require_result(&self, result: &AdmittedChannelControl) -> Result<(), Error> {
        self.require_current()?;
        result.validate()?;
        self.command
            .replay(result.command.clone())
            .map_err(|_| Error::Conflict)?;
        if result.sidecar.intent != *self.proof.intent() {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

/// Additive top-level sidecar outside the strict ordinary command record.
/// Its full source/consent evidence is a truthful historical Channel audit.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelControlSidecar {
    pub schema_version: u8,
    pub command_id: AgentCommandId,
    pub intent: ChannelControlIntent,
    pub intent_sha256: String,
    pub execution_hold: ChannelControlHold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelControlHold {
    WorkerUnavailable,
    Released,
    Refused,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedChannelControl {
    pub command: AdmittedAgentCommand,
    pub sidecar: ChannelControlSidecar,
}

impl AdmittedChannelControl {
    pub fn validate(&self) -> Result<(), Error> {
        self.command.validate().map_err(|_| Error::Corrupt)?;
        let sidecar = &self.sidecar;
        let intent = &sidecar.intent;
        intent.validate()?;
        let envelope = &self.command.receipt.envelope;
        let binding = intent.reference.binding();
        let actor_matches = matches!(&envelope.actor, AgentActor::Channel {
            bridge_id, grant_id, platform, chat_id, sender_id
        } if bridge_id == intent.callback.pin.bridge_id.as_str()
            && grant_id == binding.consent.view.grant_id.as_str()
            && platform == serde_json::to_value(&binding.source.config().connector.platform)
                .map_err(|_| Error::Corrupt)?.as_str().ok_or(Error::Corrupt)?
            && chat_id == binding.source.chat_id()
            && sender_id == &binding.sender_id);
        if sidecar.schema_version != 1
            || sidecar.intent_sha256 != intent.fingerprint()?
            || sidecar.command_id != intent.command_id()?
            || envelope.command_id != sidecar.command_id
            || envelope.command != intent.request.command.command()
            || envelope.agent_id != binding.source.target().agent_id
            || envelope.owner != intent.callback.owner
            || !actor_matches
            || envelope.source.kind != AgentSourceKind::Channel
            || envelope.source.reference != intent.id
            || envelope.correlation_id != intent.callback.callback_id
            || envelope.authorization.authority_id
                != format!("bridge_invoke:{}", binding.consent.view.grant_id.as_str())
            || envelope.authorization.grant_revision
                != Some(binding.consent_receipt.request.expected_grant_revision)
            || self.command.receipt.response_id.is_some()
            || crate::seconds_from_rfc3339(&self.command.receipt.admitted_at)
                != u64::try_from(intent.captured_at).ok()
            || crate::seconds_from_rfc3339(&self.command.queue_deadline)
                != u64::try_from(intent.captured_at)
                    .ok()
                    .and_then(|at| at.checked_add(300))
            || self.command.target_turn != Some(binding.turn_sequence)
            || self.command.key_sha256
                != command_key_digest(&intent.id).map_err(|_| Error::Corrupt)?
            || (sidecar.execution_hold == ChannelControlHold::WorkerUnavailable
                && (self.command.status != AgentResponseStatus::WaitingAction
                    || self.command.work != CommandWorkState::default()))
            || (sidecar.execution_hold == ChannelControlHold::Refused
                && (self.command.status != AgentResponseStatus::Failed
                    || self.command.work.dispatch.is_some()
                    || self.command.work.lease.is_some()
                    || self.command.work.observation.is_some()
                    || self.command.work.failure.is_none()))
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}
