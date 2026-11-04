use super::*;

#[test]
fn failed_open_requires_exact_atomic_settlement_of_the_undispatched_message() {
    let (mut current, worktree) = claimed(
        AgentWorkStep::OpenConversation,
        AgentStatus::Starting,
        WorktreeStatus::Creating,
    );
    current.work.receipts = vec![
        receipt(&current, AgentWorkStep::AddWorktree),
        receipt(&current, AgentWorkStep::MaterializeHome),
    ];
    current
        .work
        .cleanup_debt
        .push(debt(&current, AgentWorkStep::OpenConversation));
    current.conversation.first_message =
        Some(FirstMessageRecord::new("first", "Wait for input", NOW).unwrap());
    let mut next = current.clone();
    next.conversation
        .first_message
        .as_mut()
        .unwrap()
        .fail_startup(AT.into())
        .unwrap();
    next.work
        .advance("worker", fence(&current), AgentWorkStep::None, AT.into())
        .unwrap();
    let failure = typed("agent_runtime_unsupported");
    next.fail(failure.clone(), AT.into()).unwrap();
    let mut failed_worktree = worktree.clone();
    failed_worktree.transition(WorktreeStatus::Failed).unwrap();
    let failed = publication(
        &current,
        next,
        vec![AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::OpenConversation,
            failure,
        }],
        Some(SandboxEventKind::AgentStatusChanged {
            agent_id: current.id.clone(),
            from: AgentStatus::Starting,
            to: AgentStatus::Failed,
        }),
        Some(versioned(&worktree, failed_worktree)),
    );
    assert_valid(&current, &worktree, &failed);
    for mutation in ["queued", "removed", "timestamp", "turn", "input"] {
        let mut invalid = failed.clone();
        match mutation {
            "queued" => {
                invalid.agent.conversation.first_message =
                    current.conversation.first_message.clone()
            }
            "removed" => invalid.agent.conversation.first_message = None,
            "timestamp" => {
                invalid
                    .agent
                    .conversation
                    .first_message
                    .as_mut()
                    .unwrap()
                    .failed_at = Some(RETRY_AT.into())
            }
            "turn" => {
                invalid
                    .agent
                    .conversation
                    .first_message
                    .as_mut()
                    .unwrap()
                    .turn_sequence = Some(1)
            }
            "input" => {
                invalid
                    .agent
                    .conversation
                    .first_message
                    .as_mut()
                    .unwrap()
                    .text = "Changed input".into()
            }
            _ => unreachable!(),
        }
        assert_conflict(&current, &worktree, &invalid);
    }
}

#[test]
fn startup_failure_cannot_relabel_an_attempted_first_message_as_undispatched() {
    let mut first = FirstMessageRecord::new("first", "Wait for input", NOW).unwrap();
    first.fence_dispatch(7, 2, AT.into()).unwrap();
    let original = first.clone();
    assert!(first.fail_startup(RETRY_AT.into()).is_err());
    assert_eq!(first, original);
    first.retry_after_pre_write_failure().unwrap();
    let original = first.clone();
    assert!(first.fail_startup(RETRY_AT.into()).is_err());
    assert_eq!(first, original);
}
