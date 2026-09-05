use super::*;

#[tokio::test]
async fn agent_control_delete_schedules_only_stopped_or_failed_without_publishing_deleted() {
    for status in [AgentStatus::Stopped, AgentStatus::Failed] {
        let store = ready_store(1).await;
        let creation = created(store.admit_agent(admission(status.name())).await.unwrap());
        let original = terminal_agent(creation.agent, status);
        replace_agent(&store, original.clone());
        let scheduled = store
            .schedule_agent_delete(delete_request(&original))
            .await
            .unwrap()
            .into_agent();
        assert_eq!(scheduled.status, status);
        assert_eq!(scheduled.deleted_at, None);
        assert_eq!(scheduled.work.step, AgentWorkStep::EnsureProcessClosed);
        assert_eq!(scheduled.work.ready_at, MINUTE);
        assert_eq!(scheduled.updated_at, MINUTE);
        assert_eq!(scheduled.revision, original.revision + 1);
    }

    for status in [
        AgentStatus::Starting,
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
        AgentStatus::Paused,
    ] {
        let store = ready_store(1).await;
        let creation = created(store.admit_agent(admission(status.name())).await.unwrap());
        let mut agent = creation.agent;
        agent.status = status;
        agent.work.schedule(AgentWorkStep::None, NOW.into());
        replace_agent(&store, agent.clone());
        assert_eq!(
            store
                .schedule_agent_delete(delete_request(&agent))
                .await
                .unwrap_err(),
            AgentControlStoreError::StatusConflict { status }
        );
    }
}

#[tokio::test]
async fn agent_control_delete_replays_every_delete_step_and_already_deleted() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("delete-replay")).await.unwrap());
    for step in [
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
    ] {
        let mut agent = terminal_agent(creation.agent.clone(), AgentStatus::Stopped);
        agent.work.schedule(step, NOW.into());
        replace_agent(&store, agent.clone());
        assert_eq!(
            store
                .schedule_agent_delete(delete_request(&agent))
                .await
                .unwrap(),
            AgentControlResult::Existing(agent)
        );
    }

    let deleted = terminal_agent(creation.agent, AgentStatus::Deleted);
    replace_agent(&store, deleted.clone());
    assert_eq!(
        store
            .schedule_agent_delete(delete_request(&deleted))
            .await
            .unwrap(),
        AgentControlResult::Existing(deleted)
    );
}

#[tokio::test]
async fn agent_control_delete_compatibility_is_explicit_for_every_work_step() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("delete-work-compatibility"))
            .await
            .unwrap(),
    );
    for step in all_work_steps() {
        let mut agent = terminal_agent(creation.agent.clone(), AgentStatus::Stopped);
        agent.work.schedule(step, NOW.into());
        replace_agent(&store, agent.clone());
        let result = store.schedule_agent_delete(delete_request(&agent)).await;
        match step {
            AgentWorkStep::None => {
                assert!(matches!(result, Ok(AgentControlResult::Scheduled(_))));
            }
            AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted => {
                assert_eq!(result.unwrap(), AgentControlResult::Existing(agent));
            }
            _ => {
                assert_eq!(
                    result.unwrap_err(),
                    AgentControlStoreError::WorkConflict { step }
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_control_concurrent_identical_delete_requests_mutate_once() {
    let store = Arc::new(ready_store(1).await);
    let creation = created(store.admit_agent(admission("delete-race")).await.unwrap());
    let stopped = terminal_agent(creation.agent, AgentStatus::Stopped);
    replace_agent(&store, stopped.clone());
    let request = delete_request(&stopped);
    let mut tasks = Vec::new();
    for _ in 0..32 {
        let store = Arc::clone(&store);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            store.schedule_agent_delete(request).await.unwrap()
        }));
    }
    let mut scheduled = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), AgentControlResult::Scheduled(_)) {
            scheduled += 1;
        }
    }
    assert_eq!(scheduled, 1);
    let stored = store.get_agent(&stopped.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, stopped.revision + 1);
    assert_eq!(stored.work.step, AgentWorkStep::EnsureProcessClosed);
}

#[tokio::test]
async fn agent_control_failed_agent_can_publish_deleted_without_a_conversation() {
    let store = ready_store(1).await;
    let creation = created(
        store
            .admit_agent(admission("delete-before-conversation"))
            .await
            .unwrap(),
    );
    let failed = terminal_agent(creation.agent, AgentStatus::Failed);
    replace_agent(&store, failed.clone());
    let scheduled = store
        .schedule_agent_delete(delete_request(&failed))
        .await
        .unwrap()
        .into_agent();
    let claimed = store
        .claim_agent_work(
            &scheduled.id,
            "delete-worker",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut deleted = claimed.clone();
    deleted
        .work
        .advance(
            "delete-worker",
            fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "delete-worker".into(),
            fence,
            agent: deleted.clone(),
            worktree: None,
            appended_at: TWO_MINUTES.into(),
            sandbox_event: None,
            agent_logs: vec![],
        })
        .await
        .unwrap();
    assert_eq!(deleted.conversation.conversation_id, None);
    assert_eq!(store.get_agent(&deleted.id).await.unwrap(), Some(deleted));
}

#[tokio::test]
async fn agent_control_delete_keeps_logs_through_scheduling_and_terminal_publication() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("delete-logs")).await.unwrap());
    let stopped = terminal_agent(creation.agent, AgentStatus::Stopped);
    replace_agent(&store, stopped.clone());
    let scheduled = store
        .schedule_agent_delete(delete_request(&stopped))
        .await
        .unwrap()
        .into_agent();
    assert_eq!(
        store.agent_log(&scheduled.id, 0, 10).await.unwrap().len(),
        1
    );

    let claimed = store
        .claim_agent_work(
            &scheduled.id,
            "delete-worker",
            MINUTE.into(),
            TWO_MINUTES.into(),
        )
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut deleted = claimed.clone();
    deleted
        .work
        .advance(
            "delete-worker",
            fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    deleted.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed.revision,
            holder: "delete-worker".into(),
            fence,
            agent: deleted.clone(),
            worktree: None,
            appended_at: TWO_MINUTES.into(),
            sandbox_event: Some(SandboxEventKind::AgentDeleted {
                agent_id: deleted.id.clone(),
            }),
            agent_logs: vec![AgentLogKind::Deleted],
        })
        .await
        .unwrap();

    let logs = store.agent_log(&deleted.id, 0, 10).await.unwrap();
    assert_eq!(logs.len(), 2);
    assert!(matches!(logs[0].kind, AgentLogKind::Created { .. }));
    assert_eq!(logs[1].kind, AgentLogKind::Deleted);
    assert_eq!(
        store
            .schedule_agent_delete(delete_request(&deleted))
            .await
            .unwrap(),
        AgentControlResult::Existing(deleted)
    );
    assert_eq!(
        store.agent_log(&logs[0].agent_id, 0, 10).await.unwrap(),
        logs
    );
    assert_eq!(
        store
            .sandbox_events(&stopped.sandbox_id, 0, 10)
            .await
            .unwrap()
            .last()
            .unwrap()
            .kind,
        SandboxEventKind::AgentDeleted {
            agent_id: stopped.id
        }
    );
}
