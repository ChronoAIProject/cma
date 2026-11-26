//! Owner-consented settlement for a proved suspended native runtime.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    retained_recovery::RetainedRecoverySource,
    retained_reopen::{digest, RetainedReopenReplay},
    runtime_recovery_agents::recover_agent,
    store::{
        SandboxRuntimeLossAgentMutation, SandboxRuntimeLossMutation, SandboxRuntimeLossOutcome,
        SandboxStoreError,
    },
    AgentOperationKind, AgentSkillInstallStatus, AgentStatus, AgentWorkStep,
    RetainedReopenAccepted, RetainedReopenSnapshot, RuntimeBinding, SandboxEventKind,
    SandboxReviveCause, SandboxStatus, SandboxWorkStep,
};

impl RetainedReopenSnapshot {
    pub fn suspended_source(&self) -> Result<RetainedRecoverySource<'_>, SandboxStoreError> {
        let source = self.retained.native_source().map_err(|_| refused())?;
        self.validate_common()?;
        let parent = &self.retained.sandbox;
        if !source.binding.runtime_id.starts_with("agent-sandbox:")
            || parent.lifecycle.failed_provision.is_some()
            || !self.retained.agents.iter().any(|agent| {
                matches!(
                    agent.status,
                    AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
                )
            })
        {
            return Err(refused());
        }
        let mut ids = BTreeSet::new();
        for workspace in &self.retained.workspaces {
            workspace
                .validate_for_admission(parent, SandboxStatus::Failed)
                .map_err(|_| refused())?;
            if !ids.insert(&workspace.id)
                || workspace.work.lease.is_some()
                || workspace.work.step.is_pending()
            {
                return Err(refused());
            }
        }
        let mut ids = BTreeSet::new();
        for agent in &self.retained.agents {
            agent.validate_conversation().map_err(|_| refused())?;
            if !ids.insert(&agent.id)
                || agent.owner != parent.owner
                || agent.sandbox_id != parent.id
                || agent.failure.is_some()
                || agent.work.lease.is_some()
                || agent.work.step != AgentWorkStep::None
                || !agent.work.cleanup_debt.is_empty()
                || agent.requested_stop_reason.is_some()
                || agent
                    .work
                    .live_skills
                    .iter()
                    .any(|skill| skill.status == AgentSkillInstallStatus::Pending)
            {
                return Err(refused());
            }
            if agent.status == AgentStatus::Stopped {
                continue;
            }
            if !matches!(
                agent.status,
                AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused
            ) || agent.conversation.runtime_generation != Some(source.generation)
                || agent
                    .conversation
                    .conversation_id
                    .as_ref()
                    .is_none_or(|id| id.trim().is_empty())
                || agent.conversation.operation.as_ref().is_some_and(|op| {
                    op.runtime_generation > source.generation
                        || op.blocks_dispatch()
                            && (op.runtime_generation != source.generation
                                || !matches!(
                                    op.intent.kind(),
                                    AgentOperationKind::SendMessage
                                        | AgentOperationKind::Steer
                                        | AgentOperationKind::Interrupt
                                        | AgentOperationKind::AnswerApproval
                                ))
                })
            {
                return Err(refused());
            }
        }
        Ok(source)
    }
}

/// Produced only after the native provider verifies the exact suspended graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSuspendedReopenProof {
    pub binding: RuntimeBinding,
    pub home_identity: String,
}

impl RetainedSuspendedReopenProof {
    pub fn validate(&self, snapshot: &RetainedReopenSnapshot) -> Result<(), SandboxStoreError> {
        let source = snapshot.suspended_source()?;
        if self.binding != *source.binding || self.home_identity != source.home_identity {
            return Err(refused());
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RetainedSuspendedReopenRequest {
    pub snapshot: RetainedReopenSnapshot,
    pub proof: RetainedSuspendedReopenProof,
    pub key_sha256: String,
    pub requested_at: String,
    pub repeat_authorized: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedSuspendedReopenReceipt {
    pub version: u32,
    pub sandbox_id: crate::SandboxId,
    pub owner: crate::SubjectId,
    pub expected_revision: u64,
    pub key_sha256: String,
    pub snapshot_sha256: String,
    pub settled_sha256: String,
    pub authority_generation: u64,
    pub event_seq: u64,
    pub proof: RetainedSuspendedReopenProof,
    pub accepted: RetainedReopenAccepted,
}

impl From<RetainedSuspendedReopenReceipt> for RetainedReopenReplay {
    fn from(receipt: RetainedSuspendedReopenReceipt) -> Self {
        Self {
            owner: receipt.owner,
            expected_revision: receipt.expected_revision,
            key_sha256: receipt.key_sha256,
            accepted: receipt.accepted,
        }
    }
}

pub fn apply_suspended_reopen(
    current: &RetainedReopenSnapshot,
    request: &RetainedSuspendedReopenRequest,
) -> Result<(SandboxRuntimeLossMutation, RetainedSuspendedReopenReceipt), SandboxStoreError> {
    if current != &request.snapshot || !hex(&request.key_sha256) {
        return Err(SandboxStoreError::Conflict);
    }
    request.proof.validate(current)?;
    let at = &request.requested_at;
    let previous = &current.retained.sandbox;
    let requested = crate::seconds_from_rfc3339(at).ok_or_else(refused)?;
    if crate::seconds_from_rfc3339(&previous.updated_at).is_none_or(|time| time > requested)
        || current.retained.agents.iter().any(|agent| {
            crate::seconds_from_rfc3339(&agent.updated_at).is_none_or(|time| time > requested)
        })
    {
        return Err(refused());
    }
    let mut changes = Vec::new();
    for agent in current
        .retained
        .agents
        .iter()
        .filter(|agent| agent.status.is_live())
    {
        // A suspended reopen never retires, so native retirement finality is inert.
        let (next, logs) = recover_agent(previous, agent, at, false).map_err(|_| refused())?;
        if next.status != AgentStatus::Paused {
            return Err(refused());
        }
        changes.push(SandboxRuntimeLossAgentMutation {
            expected_revision: agent.revision,
            expected_last_seq: agent.conversation.last_seq,
            agent: next,
            logs,
        });
    }
    let mut parent = previous.clone();
    parent.lifecycle.suspended_runtime = parent.runtime.take().map(Box::new);
    parent
        .lifecycle
        .start_revive(&parent.id, at.clone(), SandboxReviveCause::RuntimeLost)
        .map_err(|_| refused())?;
    let operation = parent
        .lifecycle
        .operation
        .as_ref()
        .ok_or_else(refused)?
        .authority();
    let accepted = RetainedReopenAccepted {
        operation_id: operation.operation_id.clone(),
        revision: previous.revision.checked_add(1).ok_or_else(refused)?,
        accepted_at: at.clone(),
    };
    let authority_generation = operation.generation;
    parent.status = SandboxStatus::Reviving;
    parent.failure = None;
    parent.lifecycle.paused_notice =
        Some("Reopening your original conversations on the same saved environment.".into());
    parent
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, at.clone());
    parent.updated_at = at.clone();
    parent.revision = accepted.revision;
    parent.validate().map_err(|_| refused())?;
    let settled = changes
        .iter()
        .map(|change| (&change.agent, &change.logs))
        .collect::<Vec<_>>();
    let receipt = RetainedSuspendedReopenReceipt {
        version: 2,
        sandbox_id: previous.id.clone(),
        owner: previous.owner.clone(),
        expected_revision: previous.revision,
        key_sha256: request.key_sha256.clone(),
        snapshot_sha256: current.digest()?,
        settled_sha256: digest(&(&parent, settled))?,
        authority_generation,
        event_seq: current.event_seq.checked_add(1).ok_or_else(refused)?,
        proof: request.proof.clone(),
        accepted,
    };
    Ok((
        SandboxRuntimeLossMutation {
            expected_revision: previous.revision,
            sandbox: parent,
            agents: changes,
            events: vec![SandboxEventKind::StatusChanged {
                from: SandboxStatus::Failed,
                to: SandboxStatus::Reviving,
            }],
            outcome: SandboxRuntimeLossOutcome::RecoveryScheduled,
        },
        receipt,
    ))
}

fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn refused() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("suspended retained reopening is not proved")
}
