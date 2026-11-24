use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};

use crate::sandbox::{
    Agent, AgentCleanupDebt, AgentLogKind, AgentStatus, AgentWorkReceipt, AgentWorkStep,
    SandboxEventKind, Worktree,
};

use super::agent_work_prepared_pair::{paired_completion, paired_logs};
use super::agent_work_transition_validation::PublicationKind;
use super::agent_work_validation::stamp;
use super::{AgentWorkMutation, SandboxStoreError};

pub const MAX_AGENT_WORK_RECEIPTS: usize = 16;
pub const MAX_AGENT_CLEANUP_DEBT: usize = 8;
const MAX_OPERATION_ID_BYTES: usize = 256;

pub(super) fn startup_effect(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
    )
}

pub(super) fn validate_histories(
    current: &Agent,
    next: &Agent,
    appended_at: &str,
) -> Result<(), SandboxStoreError> {
    validate_receipts(current, appended_at)?;
    validate_receipts(next, appended_at)?;
    validate_debts(current)?;
    validate_debts(next)
}

fn validate_receipts(agent: &Agent, appended_at: &str) -> Result<(), SandboxStoreError> {
    if agent.work.receipts.len() > MAX_AGENT_WORK_RECEIPTS {
        return Err(SandboxStoreError::Conflict);
    }
    let ceiling = stamp(appended_at)?;
    let open_incarnation = agent
        .open_process_incarnation()
        .map_err(|_| SandboxStoreError::Conflict)?;
    // Initial startup cannot acquire authority over a replacement process.
    if agent.work.step == AgentWorkStep::OpenConversation
        && open_incarnation.is_some_and(|incarnation| incarnation > 1)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let mut prior_rank = None;
    let mut prior_stamp = None;
    for (index, receipt) in agent.work.receipts.iter().enumerate() {
        let rank = effect_rank(receipt.step).ok_or(SandboxStoreError::Conflict)?;
        let completed = stamp(&receipt.completed_at)?;
        if !valid_evidence(
            agent,
            receipt.step,
            &receipt.operation_id,
            &receipt.request_sha256,
        ) || (closes_process(receipt.step)
            && open_incarnation
                .is_some_and(|incarnation| !matches_process_close(agent, receipt, incarnation)))
            || prior_rank.is_some_and(|prior| prior >= rank)
            || prior_stamp.is_some_and(|prior| prior > completed)
            || completed > ceiling
            || agent.work.receipts[..index]
                .iter()
                .any(|prior| prior.step == receipt.step)
        {
            return Err(SandboxStoreError::Conflict);
        }
        prior_rank = Some(rank);
        prior_stamp = Some(completed);
    }
    Ok(())
}

fn validate_debts(agent: &Agent) -> Result<(), SandboxStoreError> {
    if agent.work.cleanup_debt.len() > MAX_AGENT_CLEANUP_DEBT {
        return Err(SandboxStoreError::Conflict);
    }
    let mut prior_rank = None;
    for (index, debt) in agent.work.cleanup_debt.iter().enumerate() {
        let rank = effect_rank(debt.step).ok_or(SandboxStoreError::Conflict)?;
        if !startup_effect(debt.step)
            || !valid_evidence(agent, debt.step, &debt.operation_id, &debt.request_sha256)
            || agent
                .work
                .receipts
                .iter()
                .any(|receipt| receipt.step == debt.step)
            || prior_rank.is_some_and(|prior| prior >= rank)
            || agent.work.cleanup_debt[..index]
                .iter()
                .any(|prior| prior.step == debt.step)
        {
            return Err(SandboxStoreError::Conflict);
        }
        prior_rank = Some(rank);
    }
    Ok(())
}

pub(super) fn validate_advance_history(
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    retired_process_absent: bool,
) -> Result<(), SandboxStoreError> {
    if paired_completion(current, next) {
        return Ok(());
    }
    let receipts = &next.work.receipts;
    if !receipts.starts_with(&current.work.receipts)
        || receipts.len() > current.work.receipts.len() + 1
    {
        return Err(SandboxStoreError::Conflict);
    }
    let appended = receipts.get(current.work.receipts.len());
    if (receipt_required(current) && !retired_process_absent) != appended.is_some() {
        return Err(SandboxStoreError::Conflict);
    }
    let mut expected_debt = current.work.cleanup_debt.clone();
    if let Some(receipt) = appended {
        if receipt.step != current.work.step
            || receipt.completed_at != mutation.appended_at
            || (closes_process(receipt.step)
                && !current.open_process_incarnation().is_ok_and(|open| {
                    open.is_some_and(|incarnation| {
                        matches_process_close(current, receipt, incarnation)
                    })
                }))
        {
            return Err(SandboxStoreError::Conflict);
        }
        if startup_effect(receipt.step)
            && !expected_debt
                .iter()
                .any(|debt| evidence_matches(debt, receipt))
        {
            return Err(SandboxStoreError::Conflict);
        }
        if let Some(index) = expected_debt
            .iter()
            .position(|debt| debt.step == receipt.step)
        {
            if !evidence_matches(&expected_debt[index], receipt) {
                return Err(SandboxStoreError::Conflict);
            }
            expected_debt.remove(index);
        }
        if let Some(source) = compensated_step(receipt.step) {
            expected_debt.retain(|debt| debt.step != source);
        }
    }
    if next.work.cleanup_debt == expected_debt {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn receipt_required(agent: &Agent) -> bool {
    match agent.work.step {
        step if startup_effect(step) => true,
        AgentWorkStep::CloseProcess => has_startup_evidence(agent, AgentWorkStep::OpenConversation),
        AgentWorkStep::EnsureProcessClosed => {
            !has_receipt(agent, AgentWorkStep::CloseProcess)
                && has_startup_evidence(agent, AgentWorkStep::OpenConversation)
        }
        AgentWorkStep::RemoveHome => has_startup_evidence(agent, AgentWorkStep::MaterializeHome),
        AgentWorkStep::RemoveWorktree => has_startup_evidence(agent, AgentWorkStep::AddWorktree),
        _ => false,
    }
}

fn has_receipt(agent: &Agent, step: AgentWorkStep) -> bool {
    agent
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == step)
}

fn has_startup_evidence(agent: &Agent, step: AgentWorkStep) -> bool {
    has_receipt(agent, step) || agent.work.cleanup_debt.iter().any(|debt| debt.step == step)
}

fn evidence_matches(debt: &AgentCleanupDebt, receipt: &AgentWorkReceipt) -> bool {
    debt.step == receipt.step
        && debt.operation_id == receipt.operation_id
        && debt.request_sha256 == receipt.request_sha256
}

fn compensated_step(step: AgentWorkStep) -> Option<AgentWorkStep> {
    match step {
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => {
            Some(AgentWorkStep::OpenConversation)
        }
        AgentWorkStep::RemoveHome => Some(AgentWorkStep::MaterializeHome),
        AgentWorkStep::RemoveWorktree => Some(AgentWorkStep::AddWorktree),
        _ => None,
    }
}

fn effect_rank(step: AgentWorkStep) -> Option<u8> {
    match step {
        AgentWorkStep::AddWorktree => Some(0),
        AgentWorkStep::MaterializeHome => Some(1),
        AgentWorkStep::OpenConversation => Some(2),
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => Some(3),
        AgentWorkStep::RemoveHome => Some(4),
        AgentWorkStep::RemoveWorktree => Some(5),
        _ => None,
    }
}

fn valid_evidence(agent: &Agent, step: AgentWorkStep, operation_id: &str, sha256: &str) -> bool {
    if effect_rank(step).is_none()
        || operation_id.trim().is_empty()
        || operation_id.len() > MAX_OPERATION_ID_BYTES
        || !valid_sha256(sha256)
    {
        return false;
    }
    let expected = match step {
        AgentWorkStep::MaterializeHome => Some(format!("agent:{}:home:create:v1", agent.id)),
        AgentWorkStep::OpenConversation => {
            return AgentProcessAction::Open
                .incarnation(agent.id.as_str(), operation_id)
                .is_some();
        }
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => {
            return match AgentProcessAction::Close.incarnation(agent.id.as_str(), operation_id) {
                Some(1) => true,
                Some(_) => {
                    AgentCloseRequest::new(operation_id, AgentSlot::new(agent.id.as_str()))
                        .request_sha256
                        == sha256
                }
                None => false,
            };
        }
        AgentWorkStep::RemoveHome => Some(format!("agent:{}:home:remove:v1", agent.id)),
        _ => None,
    };
    expected.is_none_or(|expected| expected == operation_id)
}

fn closes_process(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed
    )
}

fn matches_process_close(agent: &Agent, receipt: &AgentWorkReceipt, incarnation: u64) -> bool {
    AgentProcessAction::Close.incarnation(agent.id.as_str(), &receipt.operation_id)
        == Some(incarnation)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn validate_evidence(
    current: &Agent,
    worktree: &Worktree,
    mutation: &AgentWorkMutation,
    kind: PublicationKind,
) -> Result<(), SandboxStoreError> {
    let next = &mutation.agent;
    let logs_match = match kind {
        PublicationKind::Intent => mutation.agent_logs.is_empty(),
        PublicationKind::StartupNotice => matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::SignInRequired { url, code, message }]
                if !url.is_empty() && valid_notice_text(url, 4096)
                    && code.as_ref().is_none_or(|value| valid_notice_text(value, 512))
                    && message.as_ref().is_none_or(|value| valid_notice_text(value, 4096))
        ),
        PublicationKind::Retry => {
            // Stores separately prove the retained marker in the same snapshot
            // before accepting an empty cleanup retry publication.
            super::agent_cleanup_capacity_retry(current, mutation)
                || (current.work.step == AgentWorkStep::WaitForWorkspace
                    && current.has_no_runtime_effects()
                    && mutation.agent_logs.is_empty())
                || matches!(
                    mutation.agent_logs.as_slice(),
                    [AgentLogKind::WorkStepFailed { step, .. }] if *step == current.work.step
                )
        }
        PublicationKind::Advance => {
            if paired_completion(current, next) {
                paired_logs(mutation)
            } else {
                matches!(
                    mutation.agent_logs.as_slice(),
                    [AgentLogKind::WorkStepCompleted { step }] if *step == current.work.step
                )
            }
        }
        PublicationKind::Ready => matches!(
            mutation.agent_logs.as_slice(),
            [
                AgentLogKind::ConversationOpened { conversation_id, runtime_generation },
                AgentLogKind::StatusChanged { from: AgentStatus::Starting, to: AgentStatus::Ready }
            ] if Some(conversation_id) == next.conversation.conversation_id.as_ref()
                && Some(*runtime_generation) == next.conversation.runtime_generation
        ),
        PublicationKind::StartupFailure => matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::WorkStepFailed { step, failure }]
                if *step == current.work.step && Some(failure) == next.failure.as_ref()
        ),
        PublicationKind::Stop => matches!(
            mutation.agent_logs.as_slice(),
            [AgentLogKind::Stopped { reason }]
                | [AgentLogKind::TurnInterrupted { .. }, AgentLogKind::Stopped { reason }]
                if next.stop_reason.as_ref().is_some_and(|stop| stop.as_str() == reason)
        ),
        PublicationKind::Delete => {
            matches!(mutation.agent_logs.as_slice(), [AgentLogKind::Deleted])
        }
    };
    let event_matches = match kind {
        PublicationKind::Ready | PublicationKind::StartupFailure | PublicationKind::Stop => {
            matches!(
                mutation.sandbox_event.as_ref(),
                Some(SandboxEventKind::AgentStatusChanged { agent_id, from, to })
                    if agent_id == &next.id && *from == current.status && *to == next.status
            )
        }
        PublicationKind::Delete => matches!(
            mutation.sandbox_event.as_ref(),
            Some(SandboxEventKind::AgentDeleted { agent_id }) if agent_id == &next.id
        ),
        PublicationKind::Advance if current.work.step == AgentWorkStep::RemoveWorktree => {
            matches!(
                mutation.sandbox_event.as_ref(),
                Some(SandboxEventKind::WorktreeRemoved { workspace_id, worktree_id })
                    if workspace_id == &worktree.workspace_id && worktree_id == &worktree.id
            )
        }
        _ => mutation.sandbox_event.is_none(),
    };
    if logs_match && event_matches {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

fn valid_notice_text(value: &str, limit: usize) -> bool {
    value.len() <= limit
        && !value
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\t'))
}
