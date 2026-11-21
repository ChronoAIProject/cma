use super::*;
use layer2_core::sandbox::store::{plan_agent_log_capacity_failure, AGENT_LOG_CAPACITY_FAILURE};

fn fixture() -> (Agent, Worktree) {
    let mut current = conversation_agent_for_rules("agt_capacity_settlement");
    current = ready_publication_claim(current, "retained-conversation");
    current.work.step = AgentWorkStep::PublishConnection;
    current.work.lease = None;
    current
        .work
        .claim("worker".into(), NOW, MINUTE.into())
        .unwrap();
    current.conversation.last_seq = 10_000;
    let (mut worktree, _) = worktree_and_agent(&current.sandbox_id);
    worktree.id = current.worktree_id.clone();
    worktree.agent_id = Some(current.id.clone());
    worktree.workspace_id = current.workspace_id.clone();
    worktree.owner = current.owner.clone();
    worktree.status = WorktreeStatus::Ready;
    (current, worktree)
}

#[test]
fn capacity_failure_proposal_releases_only_claimed_work_and_retains_conversation() {
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
    ] {
        let (mut current, worktree) = fixture();
        current.status = status;
        current.conversation.active_turn = (status != AgentStatus::Ready).then_some(9_950);
        let fence = current.work.lease.as_ref().unwrap().fence;
        let publication =
            plan_agent_log_capacity_failure(&current, &worktree, "worker", fence, NOW).unwrap();
        let failed = &publication.agent;
        assert_eq!(
            publication_capacity(&current, &publication),
            AppendCapacity::StopOrFailure
        );
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(
            failed.failure.as_ref().unwrap().class,
            AGENT_LOG_CAPACITY_FAILURE
        );
        assert_eq!(failed.work.step, AgentWorkStep::None);
        assert!(failed.work.lease.is_none());
        assert!(!failed.work.is_due(TWO_MINUTES));
        assert_eq!(failed.work.receipts, current.work.receipts);
        assert_eq!(failed.work.cleanup_debt, current.work.cleanup_debt);
        assert_eq!(failed.work.next_fence, current.work.next_fence);
        let mut expected_conversation = current.conversation.clone();
        expected_conversation.last_seq += 1;
        assert_eq!(failed.conversation, expected_conversation);
        assert_eq!(failed.ready_at, current.ready_at);
        assert!(failed.stopped_at.is_none());
        assert!(failed.deleted_at.is_none());
        assert!(publication.worktree.is_none());
        failed.validate_conversation().unwrap();
        assert_eq!(
            serde_json::from_value::<Agent>(serde_json::to_value(failed).unwrap()).unwrap(),
            *failed
        );
        assert!(matches!(&publication.agent_logs[..],
            [AgentLogKind::WorkStepFailed { step: AgentWorkStep::PublishConnection, failure }]
            if failure.class == AGENT_LOG_CAPACITY_FAILURE));
    }
}

#[test]
fn capacity_failure_proposal_refuses_expiry_wrong_fence_cleanup_and_lineage_changes() {
    let (current, worktree) = fixture();
    let fence = current.work.lease.as_ref().unwrap().fence;
    for (holder, value, at) in [
        ("other", fence, NOW),
        ("worker", fence + 1, NOW),
        ("worker", fence, MINUTE),
    ] {
        assert!(matches!(
            plan_agent_log_capacity_failure(&current, &worktree, holder, value, at),
            Err(SandboxStoreError::Conflict)
        ));
    }
    for variant in 0..8 {
        let mut current = current.clone();
        let mut worktree = worktree.clone();
        match variant {
            0 => current.work.step = AgentWorkStep::CloseProcess,
            1 => current.requested_stop_reason = Some(AgentStopReason::Owner),
            2 => current.status = AgentStatus::Stopped,
            3 => current.work.lease = None,
            4 => current.updated_at = MINUTE.into(),
            5 => worktree.owner = SubjectId("other-owner".into()),
            6 => worktree.status = WorktreeStatus::Removing,
            _ => current.revision = u64::MAX,
        }
        assert!(
            matches!(
                plan_agent_log_capacity_failure(&current, &worktree, "worker", fence, NOW),
                Err(SandboxStoreError::Conflict)
            ),
            "variant {variant}"
        );
    }
}

#[test]
fn capacity_failure_proposal_can_fail_unfinished_worktree_without_removing_it() {
    let (mut current, mut worktree) = fixture();
    current.status = AgentStatus::Starting;
    current.ready_at = None;
    current.work.step = AgentWorkStep::OpenConversation;
    current.conversation.conversation_id = None;
    current.conversation.runtime_generation = None;
    worktree.status = WorktreeStatus::Creating;
    let fence = current.work.lease.as_ref().unwrap().fence;
    let publication =
        plan_agent_log_capacity_failure(&current, &worktree, "worker", fence, NOW).unwrap();
    let changed = publication.worktree.unwrap();
    assert_eq!(changed.expected_revision, worktree.revision);
    assert_eq!(changed.worktree.status, WorktreeStatus::Failed);
    assert_eq!(changed.worktree.path, worktree.path);
    assert_eq!(publication.agent.work.receipts, current.work.receipts);
}
