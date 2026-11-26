use layer3_adapter::AgentProcessAction;

use super::*;

#[test]
fn close_publication_must_match_the_completed_open() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::CloseProcess,
        AgentStatus::Ready,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut current);
    current.work.receipts.last_mut().unwrap().operation_id =
        AgentProcessAction::Open.operation_id(current.id.as_str(), 2);

    let mut mutation = close_publication(&current);
    let closed = mutation.agent.work.receipts.last_mut().unwrap();
    assert_eq!(
        closed.operation_id,
        format!("agent:{}:close:2:v1", current.id)
    );
    assert_valid(&current, &worktree, &mutation);

    let closed = mutation.agent.work.receipts.last_mut().unwrap();
    closed.operation_id = AgentProcessAction::Close.operation_id(current.id.as_str(), 1);
    assert_conflict(&current, &worktree, &mutation);

    let closed = mutation.agent.work.receipts.last_mut().unwrap();
    closed.operation_id = AgentProcessAction::Close.operation_id(current.id.as_str(), 2);
    closed.request_sha256 = "0".repeat(64);
    assert_conflict(&current, &worktree, &mutation);
}

#[test]
fn close_publication_must_match_an_uncertain_open_even_after_its_debt_is_removed() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::CloseProcess,
        AgentStatus::Starting,
        WorktreeStatus::Ready,
    );
    add_startup_receipts(&mut current);
    current.work.receipts.pop().unwrap();
    let mut pending = debt(&current, AgentWorkStep::OpenConversation);
    pending.operation_id = AgentProcessAction::Open.operation_id(current.id.as_str(), 2);
    current.work.cleanup_debt.push(pending);

    let mut mutation = close_publication(&current);
    assert!(mutation.agent.work.cleanup_debt.is_empty());
    assert_valid(&current, &worktree, &mutation);

    mutation
        .agent
        .work
        .receipts
        .last_mut()
        .unwrap()
        .operation_id = AgentProcessAction::Close.operation_id(current.id.as_str(), 1);
    assert_conflict(&current, &worktree, &mutation);
}

#[test]
fn ordinary_startup_cannot_admit_a_numbered_open() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::OpenConversation,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    add_startup_receipts(&mut current);
    current.work.receipts.pop().unwrap();
    let mut pending = debt(&current, AgentWorkStep::OpenConversation);
    let mut next = current.clone();
    next.work.cleanup_debt.push(pending.clone());
    let intent = publication(&current, next, vec![], None, None);
    assert_valid(&current, &worktree, &intent);

    pending.operation_id = AgentProcessAction::Open.operation_id(current.id.as_str(), 2);
    let mut forged = intent;
    forged.agent.work.cleanup_debt = vec![pending];
    assert_conflict(&current, &worktree, &forged);
}

fn close_publication(current: &Agent) -> AgentWorkMutation {
    let request = current.close_process_request().unwrap();
    let mut mutation = completed_advance(current, AgentWorkStep::PublishStopped);
    mutation.agent.work.cleanup_debt.clear();
    mutation.agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::CloseProcess,
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
        completed_at: AT.into(),
    });
    mutation
}
