use super::*;

#[tokio::test(start_paused = true)]
async fn bootstrap_wait_observes_real_readiness_then_startup_effects_run_once() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    tokio::join!(h.run(&agent.id, h.now), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        fixture.ready().await;
    });
    let bound = h.stored(&agent.id).await;
    assert_eq!(bound.work.step, AgentWorkStep::AddWorktree);
    assert_eq!(h.backend.requests().await.len(), before);
    assert_eq!(
        h.store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .base_commit,
        fixture.ready_workspace.resolved_commit.clone().unwrap()
    );
    assert!(h
        .worker
        .run_scheduled(&agent.id, h.now + Duration::from_secs(1))
        .await
        .unwrap());
    let ready = h.stored(&agent.id).await;
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.receipts.len(), 3);
    assert!(ready.work.cleanup_debt.is_empty());
    let snapshot = h.backend.snapshot(&h.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.homes.len(), 1);
    assert!(!h
        .worker
        .run_scheduled(&agent.id, h.now + Duration::from_secs(2))
        .await
        .unwrap());
    assert!(!h
        .store
        .agent_log(&agent.id, 0, 100)
        .await
        .unwrap()
        .iter()
        .any(|entry| matches!(entry.kind, AgentLogKind::WorkStepFailed { .. })));
}

#[tokio::test(start_paused = true)]
async fn bootstrap_stop_supersedes_wait_and_delete_finishes_without_runtime_effects() {
    let fixture = BootstrapHarness::new().await;
    let h = &fixture.harness;
    let agent = fixture.admit(None).await;
    let before = h.backend.requests().await.len();
    tokio::join!(h.run(&agent.id, h.now), async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let current = h.stored(&agent.id).await;
        h.store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&current),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(h.now),
            })
            .await
            .unwrap();
    });
    assert!(h
        .worker
        .run_one(&agent.id, h.now + Duration::from_secs(1))
        .await
        .unwrap());
    for _ in 0..2 {
        assert!(h
            .worker
            .run_one(&agent.id, h.now + Duration::from_secs(1))
            .await
            .unwrap());
    }
    let stopped = h.stored(&agent.id).await;
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped.has_no_runtime_effects());
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(h.now + Duration::from_secs(5)),
        })
        .await
        .unwrap();
    for _ in 0..4 {
        assert!(h
            .worker
            .run_one(&agent.id, h.now + Duration::from_secs(5))
            .await
            .unwrap());
    }
    let deleted = h.stored(&agent.id).await;
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert!(deleted.has_no_runtime_effects());
    assert_eq!(
        h.store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    assert_eq!(h.backend.requests().await.len(), before);
}
