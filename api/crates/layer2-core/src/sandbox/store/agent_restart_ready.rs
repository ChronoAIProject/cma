use crate::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep, SandboxEventKind, Worktree,
};

use super::{AgentWorkMutation, SandboxStoreError};

/// Build the terminal publication once the worker has proved replacement attachment.
pub fn owner_restart_ready(
    agent: &Agent,
    holder: &str,
    fence: u64,
    at: String,
) -> Result<AgentWorkMutation, SandboxStoreError> {
    let [open] = agent.work.cleanup_debt.as_slice() else {
        return Err(SandboxStoreError::Conflict);
    };
    if agent.status != AgentStatus::Starting
        || agent.work.step != AgentWorkStep::RestartConversation
        || open.step != AgentWorkStep::OpenConversation
        || agent.conversation.active_turn.is_some()
        || agent.requested_stop_reason.is_some()
        || agent.work.receipts.iter().any(|receipt| {
            matches!(
                receipt.step,
                AgentWorkStep::OpenConversation | AgentWorkStep::CloseProcess
            )
        })
        || agent.work.receipts.len() >= super::MAX_AGENT_WORK_RECEIPTS
    {
        return Err(SandboxStoreError::Conflict);
    }
    let generation = agent
        .conversation
        .runtime_generation
        .and_then(|generation| generation.checked_add(1))
        .ok_or(SandboxStoreError::Conflict)?;
    let mut next = agent.clone();
    next.conversation.runtime_generation = Some(generation);
    next.work.cleanup_debt.clear();
    next.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: open.operation_id.clone(),
        request_sha256: open.request_sha256.clone(),
        completed_at: at.clone(),
    });
    next.work
        .advance(holder, fence, AgentWorkStep::None, at.clone())
        .map_err(|_| SandboxStoreError::Conflict)?;
    next.transition(AgentStatus::Ready, at.clone(), None)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let agent_logs = vec![
        AgentLogKind::ConversationOpened {
            conversation_id: agent
                .conversation
                .conversation_id
                .clone()
                .ok_or(SandboxStoreError::Conflict)?,
            runtime_generation: generation,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    next.conversation.last_seq = next
        .conversation
        .last_seq
        .checked_add(2)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    Ok(AgentWorkMutation {
        expected_agent_revision: agent.revision,
        holder: holder.into(),
        fence,
        agent: next,
        worktree: None,
        appended_at: at,
        sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
            agent_id: agent.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        }),
        agent_logs,
    })
}

pub(super) fn validate(
    current: &Agent,
    worktree: &Worktree,
    proposed: &AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
    current
        .validate_prerequisites(worktree)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let expected = owner_restart_ready(
        current,
        &proposed.holder,
        proposed.fence,
        proposed.appended_at.clone(),
    )?;
    super::agent_work_evidence_validation::validate_histories(
        current,
        &expected.agent,
        &proposed.appended_at,
    )?;
    if proposed.agent != expected.agent
        || proposed.worktree.is_some()
        || proposed.sandbox_event != expected.sandbox_event
        || proposed.agent_logs != expected.agent_logs
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
