use super::*;
use crate::sandbox::{AgentStatus, AgentWorkStep, FirstMessageDelivery};

pub(super) fn validate_checkpoint_agents(
    sandbox: &Sandbox,
    agents: &[Agent],
    checkpoint: &SandboxCheckpointRef,
) -> Result<(), SandboxStoreError> {
    if agents.len() > MAX_SANDBOX_LIFECYCLE_AGENTS
        || agents
            .iter()
            .any(|agent| agent.sandbox_id != sandbox.id || agent.owner != sandbox.owner)
    {
        return Err(SandboxStoreError::Corrupt(
            "stored checkpoint agent lineage is inconsistent".into(),
        ));
    }
    let live = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .map(|agent| {
            if agent.conversation.active_turn.is_some()
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.blocks_dispatch())
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || agent.requested_stop_reason.is_some()
            {
                return Err(SandboxStoreError::Conflict);
            }
            let conversation_id = agent
                .conversation
                .conversation_id
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?;
            Ok((agent.id.clone(), conversation_id.clone()))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let captured = checkpoint
        .conversations
        .iter()
        .map(|conversation| {
            (
                conversation.agent_id.clone(),
                conversation.conversation_id.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    if live != captured {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

pub(super) fn validate_agent_change(
    sandbox: &Sandbox,
    current: &Agent,
    actual_last_seq: u64,
    change: &SandboxLifecycleAgentMutation,
    appended_at: &str,
    events: &[SandboxEventKind],
) -> Result<(), SandboxStoreError> {
    let next = &change.agent;
    let expected_revision = current
        .revision
        .checked_add(1)
        .ok_or(SandboxStoreError::Conflict)?;
    let expected_last_seq = current
        .conversation
        .last_seq
        .checked_add(u64::try_from(change.logs.len()).map_err(|_| SandboxStoreError::Conflict)?)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let status_transition = current.status == next.status
        || current.status.transition(next.status).ok() == Some(next.status);
    let mut expected_work = current.work.clone();
    if current.status == AgentStatus::Paused && next.status == AgentStatus::Ready {
        let receipt = next
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
            .filter(|receipt| receipt.completed_at == appended_at)
            .ok_or(SandboxStoreError::Conflict)?;
        super::super::agent_work_evidence_validation::validate_histories(
            current,
            next,
            appended_at,
        )?;
        expected_work
            .receipts
            .retain(|receipt| receipt.step != AgentWorkStep::OpenConversation);
        expected_work.receipts.push(receipt.clone());
        if current
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|message| message.state == FirstMessageDelivery::Queued)
        {
            expected_work.schedule(AgentWorkStep::DispatchFirstMessage, appended_at.into());
        } else if current
            .work
            .live_skills
            .last()
            .is_some_and(|skill| skill.status == crate::sandbox::AgentSkillInstallStatus::Pending)
        {
            expected_work.schedule(AgentWorkStep::InstallSkill, appended_at.into());
        }
    }
    if change.logs.len() > MAX_AGENT_LOG_BATCH_SIZE
        || actual_last_seq != current.conversation.last_seq
        || change.expected_revision != current.revision
        || next.revision != expected_revision
        || next.conversation.last_seq != expected_last_seq
        || next.sandbox_id != sandbox.id
        || next.owner != sandbox.owner
        || !current.status.is_live()
        || !next.status.is_live()
        || !status_transition
        || expected_work != next.work
        || current.work.step.is_pending()
        || current.work.lease.is_some()
        || current.requested_stop_reason.is_some()
        || next.updated_at != appended_at
        || !agent_identity_fields_match(current, next)
    {
        return Err(SandboxStoreError::Conflict);
    }
    next.validate_conversation()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;

    let _ = events;
    Ok(())
}

pub(super) fn agent_identity_fields_match(current: &Agent, next: &Agent) -> bool {
    current.id == next.id
        && current.worktree_id == next.worktree_id
        && current.workspace_id == next.workspace_id
        && current.creation_key == next.creation_key
        && current.creation_fingerprint == next.creation_fingerprint
        && current.profile == next.profile
        && current.failure == next.failure
        && current.title == next.title
        && current.created_at == next.created_at
        && current.ready_at == next.ready_at
        && current.stopped_at == next.stopped_at
        && current.requested_stop_reason == next.requested_stop_reason
        && current.stop_reason == next.stop_reason
        && current.deleted_at == next.deleted_at
        && current.origin == next.origin
}
