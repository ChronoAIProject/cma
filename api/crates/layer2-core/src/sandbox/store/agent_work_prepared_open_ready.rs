//! A prepared conversation may publish its completed open and readiness atomically.
use crate::sandbox::{
    Agent, AgentLogKind, AgentOrigin, AgentStatus, AgentWorkStep, Sandbox, SandboxEventKind,
    Workspace, Worktree, WorktreeStatus,
};
use crate::seconds_from_rfc3339;

use super::agent_work_evidence_validation::validate_histories;
use super::agent_work_transition_validation::validate_position;
use super::{AgentWorkMutation, SandboxStoreError};

/// Stack-local parent snapshots used by the worker's fresh runtime observation.
/// A store must compare and write-fence these in the publication transaction.
#[derive(Debug, Clone)]
pub struct PreparedAgentOpenContext {
    pub sandbox: Sandbox,
    pub workspace: Workspace,
    pub worktree: Worktree,
}

/// This validates the mutation shape, not preparation ownership or live attachment.
/// The caller retains a live staged attachment; the store proves the exact prepared
/// profile, including its immutable instructions, and fences parents.
pub fn validate_prepared_agent_open_ready(
    current: &Agent,
    tree: &Worktree,
    actual_last_seq: u64,
    mutation: &AgentWorkMutation,
) -> Result<(), SandboxStoreError> {
    super::agent_work_validation::validate_common(current, tree, actual_last_seq, mutation)?;
    validate_position(current, tree, false)?;
    validate_histories(current, &mutation.agent, &mutation.appended_at)?;
    let next = &mutation.agent;
    let [add, home] = current.work.receipts.as_slice() else {
        return Err(SandboxStoreError::Conflict);
    };
    let [debt] = current.work.cleanup_debt.as_slice() else {
        return Err(SandboxStoreError::Conflict);
    };
    let Some(receipt) = next.work.receipts.get(2) else {
        return Err(SandboxStoreError::Conflict);
    };
    if current.status != AgentStatus::Starting
        || current.work.step != AgentWorkStep::OpenConversation
        || !matches!(current.origin, AgentOrigin::Lifecycle { .. })
        || current
            .work
            .warm_conversation_id
            .as_ref()
            .is_none_or(|id| id.is_empty())
        || current.work.resolved_skills.as_deref() != Some(&[])
        || !current.profile.content.skills.is_empty()
        || current.conversation.settings.is_some()
        || current.conversation.conversation_id.is_some()
        || current.conversation.runtime_generation.is_some()
        || !current.work.live_skills.is_empty()
        || add.step != AgentWorkStep::AddWorktree
        || home.step != AgentWorkStep::MaterializeHome
        || debt.step != AgentWorkStep::OpenConversation
        || receipt.step != debt.step
        || receipt.operation_id != debt.operation_id
        || receipt.request_sha256 != debt.request_sha256
        || receipt.completed_at != mutation.appended_at
        || current.work.lease.as_ref().is_none_or(|lease| {
            seconds_from_rfc3339(&lease.expires_at).is_none_or(|expires| {
                seconds_from_rfc3339(&mutation.appended_at).is_none_or(|at| at >= expires)
            })
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut expected = current.clone();
    expected.conversation.conversation_id = current.work.warm_conversation_id.clone();
    expected.conversation.runtime_generation = Some(1);
    expected.work.cleanup_debt.clear();
    expected.work.receipts.push(receipt.clone());
    expected
        .work
        .advance(
            &mutation.holder,
            mutation.fence,
            AgentWorkStep::PublishConnection,
            mutation.appended_at.clone(),
        )
        .map_err(|_| SandboxStoreError::Conflict)?;
    expected
        .transition(AgentStatus::Ready, mutation.appended_at.clone(), None)
        .map_err(|_| SandboxStoreError::Conflict)?;
    expected.conversation.last_seq = current
        .conversation
        .last_seq
        .checked_add(3)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    let mut ready_tree = tree.clone();
    ready_tree
        .transition(WorktreeStatus::Ready)
        .map_err(|_| SandboxStoreError::Conflict)?;
    let logs = vec![
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::OpenConversation,
        },
        AgentLogKind::ConversationOpened {
            conversation_id: expected
                .conversation
                .conversation_id
                .clone()
                .ok_or(SandboxStoreError::Conflict)?,
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Starting,
            to: AgentStatus::Ready,
        },
    ];
    if next != &expected
        || mutation.agent_logs != logs
        || mutation.sandbox_event
            != Some(SandboxEventKind::AgentStatusChanged {
                agent_id: current.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Ready,
            })
        || mutation.worktree.as_ref().is_none_or(|versioned| {
            versioned.expected_revision != tree.revision || versioned.worktree != ready_tree
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}
