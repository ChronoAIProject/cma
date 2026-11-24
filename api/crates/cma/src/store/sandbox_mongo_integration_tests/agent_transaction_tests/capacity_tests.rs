use super::*;

pub(in crate::store) mod mongo;
mod retry_tests;
mod settlement;

#[test]
fn agent_log_reserves_allow_cleanup_completion_but_never_an_unbounded_retry() {
    for (from, to, status, expected) in [
        (
            AgentWorkStep::FenceInteraction,
            AgentWorkStep::CloseProcess,
            AgentStatus::Paused,
            AppendCapacity::StopOrFailure,
        ),
        (
            AgentWorkStep::CloseProcess,
            AgentWorkStep::PublishStopped,
            AgentStatus::Paused,
            AppendCapacity::StopOrFailure,
        ),
        (
            AgentWorkStep::EnsureProcessClosed,
            AgentWorkStep::RemoveHome,
            AgentStatus::Stopped,
            AppendCapacity::Delete,
        ),
        (
            AgentWorkStep::RemoveHome,
            AgentWorkStep::RemoveWorktree,
            AgentStatus::Failed,
            AppendCapacity::Delete,
        ),
        (
            AgentWorkStep::RemoveWorktree,
            AgentWorkStep::PublishDeleted,
            AgentStatus::Stopped,
            AppendCapacity::Delete,
        ),
    ] {
        let mut current = agent_for_worker("agt_cleanup_capacity", NOW);
        current.status = status;
        current.work.step = from;
        current.requested_stop_reason = status.is_live().then_some(AgentStopReason::Owner);
        let mut next = current.clone();
        next.work.step = to;
        next.updated_at = MINUTE.into();
        let mutation = AgentWorkMutation {
            expected_agent_revision: current.revision,
            holder: "worker".into(),
            fence: 1,
            agent: next,
            worktree: None,
            appended_at: MINUTE.into(),
            sandbox_event: None,
            agent_logs: vec![AgentLogKind::WorkStepCompleted { step: from }],
        };
        assert_eq!(publication_capacity(&current, &mutation), expected);

        let mut retry = mutation.clone();
        retry.agent.work.step = from;
        retry.agent_logs = vec![AgentLogKind::WorkStepFailed {
            step: from,
            failure: TypedFailure {
                class: "agent_runtime_unavailable".into(),
                detail: "The runtime is temporarily unavailable.".into(),
            },
        }];
        assert_eq!(
            publication_capacity(&current, &retry),
            AppendCapacity::Normal
        );

        let mut extra = mutation.clone();
        extra
            .agent_logs
            .push(AgentLogKind::WorkStepCompleted { step: from });
        assert_eq!(
            publication_capacity(&current, &extra),
            AppendCapacity::Normal
        );

        let mut skipped = mutation.clone();
        skipped.agent.work.step = AgentWorkStep::None;
        assert_eq!(
            publication_capacity(&current, &skipped),
            AppendCapacity::Normal
        );

        let mut wrong_evidence = mutation;
        wrong_evidence.agent_logs = vec![AgentLogKind::WorkStepCompleted { step: to }];
        assert_eq!(
            publication_capacity(&current, &wrong_evidence),
            AppendCapacity::Normal
        );
    }
}

#[test]
fn agent_log_reconnect_does_not_consume_terminal_cleanup_headroom() {
    let mut current = conversation_agent_for_rules("agt_connection_capacity");
    current.work.step = AgentWorkStep::PublishConnection;
    let mut next = current.clone();
    next.work.step = AgentWorkStep::None;
    next.updated_at = MINUTE.into();
    let mutation = AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence: 1,
        agent: next,
        worktree: None,
        appended_at: MINUTE.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection,
        }],
    };
    assert_eq!(
        publication_capacity(&current, &mutation),
        AppendCapacity::Normal
    );
}
