//! Build a terminal mutation from an exact, durably drained producer seal.
use layer3_adapter::{
    stale_turn::{SealedOutput, StaleTurnSeal, StaleTurnTarget},
    AgentSlot, ConversationUpdate, RuntimeHandle,
};
use sha2::{Digest, Sha256};

use crate::{
    agent_ownership::{OwnershipToken, PROTOCOL_VERSION},
    sandbox::{
        agent_failure_projection::AGENT_TURN_NO_PROGRESS,
        agent_io::{AdmittedAgentCommand, AgentOwner, AgentResponseStatus},
        conversation_fold::{fold_update, FoldedConversationUpdate},
        AgentLogEntry, AgentOperationDelivery, AgentOperationKind, AgentStatus, AgentUpdateOutcome,
        AgentWorkStep, Sandbox, SandboxStatus, SandboxWorkStep,
    },
    seconds_from_rfc3339,
};

use super::{
    AgentConversationMutation, AgentMessageCheckpoint, AgentMessageSnapshot, SandboxStoreError,
};

/// Exact precommit facts. The store must compare these facts under the current
/// Agent writer and fence the parent and command in the same transaction.
pub struct StaleTurnSettlement {
    snapshot: AgentMessageSnapshot,
    parent: Sandbox,
    command: AdmittedAgentCommand,
    ownership: OwnershipToken,
    proof_sha256: String,
    mutation: AgentConversationMutation,
    checkpoint: AgentMessageCheckpoint,
}

impl StaleTurnSettlement {
    pub fn snapshot(&self) -> &AgentMessageSnapshot {
        &self.snapshot
    }
    pub fn parent(&self) -> &Sandbox {
        &self.parent
    }
    pub fn command(&self) -> &AdmittedAgentCommand {
        &self.command
    }
    pub fn ownership(&self) -> &OwnershipToken {
        &self.ownership
    }
    pub fn proof_sha256(&self) -> &str {
        &self.proof_sha256
    }
    pub fn mutation(&self) -> &AgentConversationMutation {
        &self.mutation
    }
    pub fn checkpoint(&self) -> &AgentMessageCheckpoint {
        &self.checkpoint
    }

    pub fn target(
        snapshot: &AgentMessageSnapshot,
        parent: &Sandbox,
        command: &AdmittedAgentCommand,
        owner: &OwnershipToken,
    ) -> Result<StaleTurnTarget, SandboxStoreError> {
        if snapshot
            .checkpoint
            .as_ref()
            .is_some_and(|checkpoint| !checkpoint.assembly.is_empty())
        {
            return Err(SandboxStoreError::Conflict);
        }
        Self::receipt_target(snapshot, parent, command, owner)
    }

    /// Identify the receipt to read for a previously admitted stop.
    pub fn receipt_target(
        snapshot: &AgentMessageSnapshot,
        parent: &Sandbox,
        command: &AdmittedAgentCommand,
        owner: &OwnershipToken,
    ) -> Result<StaleTurnTarget, SandboxStoreError> {
        let agent = &snapshot.agent;
        let command_envelope = &command.receipt.envelope;
        let dispatch = command
            .work
            .dispatch
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let delivery = command
            .work
            .observation
            .as_ref()
            .map_or(dispatch.delivery, |observation| observation.delivery);
        let turn = agent
            .conversation
            .active_turn
            .ok_or(SandboxStoreError::Conflict)?;
        let generation = agent
            .conversation
            .runtime_generation
            .ok_or(SandboxStoreError::Conflict)?;
        let conversation_id = agent
            .conversation
            .conversation_id
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        let runtime = parent.runtime.as_ref().ok_or(SandboxStoreError::Conflict)?;
        if agent.status != AgentStatus::Working
            || agent.requested_stop_reason.is_some()
            || agent.work.cleanup_not_before.is_some()
            || agent.work.step != AgentWorkStep::None
            || owner.agent_id != agent.id
            || owner.protocol != PROTOCOL_VERSION
            || parent.id != agent.sandbox_id
            || parent.owner != agent.owner
            || parent.status != SandboxStatus::Active
            || parent.lifecycle.operation.is_some()
            || parent.work.lease.is_some()
            || !matches!(
                parent.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || command.status != AgentResponseStatus::InProgress
            || command_envelope.agent_id != agent.id
            || command_envelope.command.kind() != AgentOperationKind::SendMessage
            || !matches!(&command_envelope.owner, AgentOwner::Person { subject } if *subject == agent.owner)
            || dispatch.claim.agent_id != agent.id
            || dispatch.claim.command_id != command_envelope.command_id
            || dispatch.turn_sequence != turn
            || dispatch.runtime_generation != generation
            || !matches!(
                delivery,
                AgentOperationDelivery::WriteConfirmed | AgentOperationDelivery::ExecutionObserved
            )
            || agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.sequence != dispatch.operation_sequence
                        || operation.turn_sequence != turn
                        || operation.runtime_generation != generation
                        || operation.intent.kind() != AgentOperationKind::SendMessage
                })
            || snapshot.checkpoint.as_ref().is_none_or(|checkpoint| {
                !checkpoint.matches(agent)
                    || (!checkpoint.assembly.is_empty() && checkpoint.output.is_none())
            })
        {
            return Err(SandboxStoreError::Conflict);
        }
        let target = StaleTurnTarget {
            sandbox_id: parent.id.to_string(),
            slot: AgentSlot(agent.id.to_string()),
            runtime: RuntimeHandle(runtime.runtime_id.clone()),
            runtime_boot: runtime.boot_id.clone().ok_or(SandboxStoreError::Conflict)?,
            runtime_generation: runtime
                .provision_generation
                .ok_or(SandboxStoreError::Conflict)?,
            conversation_id: conversation_id.clone(),
            conversation_generation: generation,
            command_id: command_envelope.command_id.to_string(),
            operation_sequence: dispatch.operation_sequence,
            turn_sequence: turn,
            writer_epoch: owner.epoch,
            stream_id: snapshot
                .checkpoint
                .as_ref()
                .and_then(|value| value.output.as_ref())
                .map(|output| output.stream_id.clone()),
        };
        target
            .is_valid()
            .then_some(target)
            .ok_or(SandboxStoreError::Conflict)
    }

    pub fn prepare(
        snapshot: AgentMessageSnapshot,
        parent: Sandbox,
        command: AdmittedAgentCommand,
        ownership: OwnershipToken,
        seal: StaleTurnSeal,
        appended_at: String,
    ) -> Result<Self, SandboxStoreError> {
        let target = Self::receipt_target(&snapshot, &parent, &command, &ownership)?;
        let previous = snapshot
            .checkpoint
            .as_ref()
            .ok_or(SandboxStoreError::Conflict)?;
        previous.validate()?;
        if !seal.matches(&target)
            || !seal.matches_durable_output(previous.output.as_ref())
            || (!previous.assembly.is_empty()
                && !matches!(&seal.output, SealedOutput::Through { .. }))
            || seconds_from_rfc3339(&appended_at).is_none()
        {
            return Err(SandboxStoreError::Conflict);
        }
        let proof_bytes = serde_json::to_vec(&(
            &seal,
            &previous.update_id,
            &previous.update_fingerprint,
            previous.after_seq,
            snapshot.agent.revision,
            parent.revision,
            command.revision,
        ))
        .map_err(|_| SandboxStoreError::Conflict)?;
        let proof_sha256 = format!("{:x}", Sha256::digest(&proof_bytes));
        let update = ConversationUpdate::WithoutPromptEvidence(Box::new(
            ConversationUpdate::FailedForPrompt {
                prompt_id: target.prompt(),
                message: "The Agent turn ended after it stopped making progress.".into(),
                capability: Some(AGENT_TURN_NO_PROGRESS.into()),
            },
        ));
        let current = &snapshot.agent;
        let FoldedConversationUpdate::Changed(change) =
            fold_update(current, &previous.assembly, update, &appended_at, true)
                .map_err(|_| SandboxStoreError::Conflict)?
        else {
            return Err(SandboxStoreError::Conflict);
        };
        let agent = change.agent;
        let mutation = AgentConversationMutation {
            admission_guard: None,
            settings_fence: None,
            execution_fence: None,
            trigger_execution_fence: None,
            command_claim: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: target.conversation_generation,
            agent,
            appended_at: appended_at.clone(),
            agent_logs: change.logs,
        };
        let checkpoint = AgentMessageCheckpoint {
            version: previous.version,
            update_id: uuid::Uuid::new_v4().to_string(),
            update_fingerprint: proof_sha256.clone(),
            native_conversation_id: previous.native_conversation_id.clone(),
            runtime_generation: previous.runtime_generation,
            active_turn: None,
            after_seq: mutation.agent.conversation.last_seq,
            assembly: change.assembly,
            outcome: AgentUpdateOutcome {
                turn_closed: change.turn_closed,
                appended: mutation
                    .agent_logs
                    .iter()
                    .enumerate()
                    .map(|(offset, kind)| AgentLogEntry {
                        agent_id: current.id.clone(),
                        seq: current.conversation.last_seq + offset as u64 + 1,
                        appended_at: appended_at.clone(),
                        kind: kind.clone(),
                    })
                    .collect(),
            },
            output: previous.output.clone(),
        };
        checkpoint.validate()?;
        Ok(Self {
            snapshot,
            parent,
            command,
            ownership,
            proof_sha256,
            mutation,
            checkpoint,
        })
    }
}
