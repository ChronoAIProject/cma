use super::*;

#[tokio::test]
async fn terminal_runtime_rejection_fails_agent_and_worktree_atomically() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("rejected", Vec::new()).await;
    harness.backend.fail_next(
        WireOperation::AddWorktree,
        "quota_rejected",
        "runtime rejected the worktree",
    );

    harness.run(&agent.id, harness.now).await;
    let failed = harness.stored(&agent.id).await;
    let worktree = harness
        .store
        .get_worktree(&failed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(failed.work.step, AgentWorkStep::None);
    assert_eq!(worktree.status, WorktreeStatus::Failed);
    assert_eq!(
        failed
            .failure
            .as_ref()
            .map(|failure| failure.class.as_str()),
        Some("agent_runtime_rejected")
    );
    let logs = harness.store.agent_log(&agent.id, 0, 20).await.unwrap();
    assert!(matches!(
        logs.last().map(|entry| &entry.kind),
        Some(AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::AddWorktree,
            ..
        })
    ));
}

#[tokio::test]
async fn stop_retains_resources_then_delete_removes_them_in_runtime_order() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("cleanup", Vec::new()).await;
    let ready = harness.drive_ready(&agent.id).await;
    let target = Harness::target(&ready);
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(harness.registry.resolve(&target).is_none());
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));

    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(10)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(10))
            .await;
    }
    let deleted = harness.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert_eq!(deleted.work.step, AgentWorkStep::None);
    let worktree = harness
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(worktree.status, WorktreeStatus::Removed);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert!(!snapshot
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
    assert!(!snapshot
        .worktrees
        .contains_key(&("project".into(), "wt-1".into())));
    let operations = harness.backend.requests().await[before..]
        .iter()
        .map(|(_, request)| request.operation())
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        [
            WireOperation::CloseAgent,
            WireOperation::RemoveAgentHome,
            WireOperation::RemoveWorktree,
        ]
    );
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    let stopped_index = logs
        .iter()
        .position(|entry| matches!(entry.kind, AgentLogKind::Stopped { .. }))
        .unwrap();
    let deleted_index = logs
        .iter()
        .position(|entry| matches!(entry.kind, AgentLogKind::Deleted))
        .unwrap();
    assert!(stopped_index < deleted_index);
}

#[tokio::test]
async fn deletion_after_rejected_creation_compensates_durable_intent() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("failed-cleanup", Vec::new()).await;
    harness.backend.fail_next(
        WireOperation::AddWorktree,
        "request_rejected",
        "worktree was never created",
    );
    harness.run(&agent.id, harness.now).await;
    let failed = harness.stored(&agent.id).await;
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&failed),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();

    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let deleted = harness.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert!(deleted.work.cleanup_debt.is_empty());
    assert_eq!(
        harness
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    assert_eq!(
        harness.backend.requests().await[before..]
            .iter()
            .map(|(_, request)| request.operation())
            .collect::<Vec<_>>(),
        [WireOperation::RemoveWorktree]
    );
}

#[tokio::test]
async fn stop_and_delete_before_startup_skip_every_runtime_agent_resource() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("early-stop", Vec::new()).await;
    let before = harness.backend.requests().await.len();
    harness
        .store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(harness.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(5))
            .await;
    }
    let stopped = harness.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert_eq!(
        harness
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Failed
    );
    harness
        .store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(harness.now + Duration::from_secs(10)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(10))
            .await;
    }
    assert_eq!(harness.stored(&agent.id).await.status, AgentStatus::Deleted);
    assert!(harness.backend.requests().await[before..].is_empty());
}
