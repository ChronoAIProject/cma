use super::*;

#[test]
fn agent_log_terminal_capacity_requires_a_terminal_publication() {
    let starting = agent_for_worker("agt_log_capacity", NOW);
    let publication =
        |agent: Agent,
         appended_at: &str,
         agent_logs: Vec<AgentLogKind>,
         sandbox_event: Option<SandboxEventKind>| AgentWorkMutation {
            expected_agent_revision: 1,
            holder: "worker".into(),
            fence: 1,
            agent,
            worktree: None,
            appended_at: appended_at.into(),
            sandbox_event,
            agent_logs,
        };

    let mut ready = starting.clone();
    ready.status = AgentStatus::Ready;
    let ready = publication(ready, NOW, vec![], None);
    assert_eq!(
        publication_capacity(&starting, &ready),
        AppendCapacity::Normal
    );

    let failure = TypedFailure {
        class: "startup_failed".into(),
        detail: "runtime did not start".into(),
    };
    let mut failed = starting.clone();
    failed.status = AgentStatus::Failed;
    failed.updated_at = MINUTE.into();
    failed.failure = Some(failure.clone());
    let failed = publication(
        failed,
        MINUTE,
        vec![AgentLogKind::WorkStepFailed {
            step: starting.work.step,
            failure: failure.clone(),
        }],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: starting.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Failed,
        }),
    );
    assert_eq!(
        publication_capacity(&starting, &failed),
        AppendCapacity::StopOrFailure
    );
    let mut incomplete_failure = failed.clone();
    incomplete_failure.agent_logs.clear();
    assert_eq!(
        publication_capacity(&starting, &incomplete_failure),
        AppendCapacity::Normal
    );

    let mut stopped = starting.clone();
    stopped.status = AgentStatus::Stopped;
    stopped.updated_at = MINUTE.into();
    stopped.stopped_at = Some(MINUTE.into());
    stopped.stop_reason = Some(AgentStopReason::Owner);
    let stopped = publication(
        stopped,
        MINUTE,
        vec![AgentLogKind::Stopped {
            reason: AgentStopReason::Owner.as_str().into(),
        }],
        None,
    );
    assert_eq!(
        publication_capacity(&starting, &stopped),
        AppendCapacity::StopOrFailure
    );
    let mut wrong_stop = stopped.clone();
    wrong_stop.agent_logs = vec![AgentLogKind::Stopped {
        reason: AgentStopReason::Operator.as_str().into(),
    }];
    assert_eq!(
        publication_capacity(&starting, &wrong_stop),
        AppendCapacity::Normal
    );

    let mut deleted = stopped.agent.clone();
    deleted.status = AgentStatus::Deleted;
    deleted.updated_at = TWO_MINUTES.into();
    deleted.deleted_at = Some(TWO_MINUTES.into());
    let deleted = publication(
        deleted,
        TWO_MINUTES,
        vec![AgentLogKind::Deleted],
        Some(SandboxEventKind::AgentDeleted {
            agent_id: starting.id.clone(),
        }),
    );
    assert_eq!(
        publication_capacity(&stopped.agent, &deleted),
        AppendCapacity::Delete
    );
    let mut duplicate_delete = deleted.clone();
    duplicate_delete.agent_logs.push(AgentLogKind::Deleted);
    assert_eq!(
        publication_capacity(&stopped.agent, &duplicate_delete),
        AppendCapacity::Normal
    );
    assert_eq!(
        publication_capacity(&starting, &deleted),
        AppendCapacity::Normal
    );
}

#[test]
fn mongo_agent_conversation_validation_fences_every_durable_boundary() {
    let current = conversation_agent_for_rules("agt_conversation_rules");
    let mutation = begin_conversation_turn(current.clone());
    assert!(validate_mutation(&current, 2, &mutation).is_ok());

    let rejected = |mutation: &AgentConversationMutation, actual_last_seq| {
        assert_eq!(
            validate_mutation(&current, actual_last_seq, mutation),
            Err(SandboxStoreError::Conflict)
        );
    };
    let mut invalid = mutation.clone();
    invalid.appended_at = "not-a-timestamp".into();
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.runtime_generation = 0;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.expected_agent_revision += 1;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.expected_last_seq += 1;
    rejected(&invalid, 2);
    rejected(&mutation, 1);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.last_seq += 1;
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.conversation_id = Some("different-conversation".into());
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.conversation.runtime_generation = Some(2);
    rejected(&invalid, 2);
    let mut invalid = mutation.clone();
    invalid.agent.requested_stop_reason = Some(AgentStopReason::Owner);
    rejected(&invalid, 2);
    let mut invalid = mutation;
    invalid.agent_logs.clear();
    rejected(&invalid, 2);
}
