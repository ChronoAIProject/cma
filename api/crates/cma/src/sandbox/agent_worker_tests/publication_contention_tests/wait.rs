use super::*;

#[tokio::test(start_paused = true)]
async fn wait_bind_retries_publication_then_adds_worktree_once() {
    for change in [Change::None, Change::Title, Change::Log] {
        let mut fixture = Waiting::new().await;
        let agent = fixture.admit().await;
        fixture.ready().await;
        let h = &mut fixture.h;
        let claimed = claim(h, &agent.id).await;
        let before = h.backend.requests().await.len();
        let store = inject(
            h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Any,
            Fault::Abort(change),
        );
        h.worker
            .execute_claimed_for_test(claimed, h.now)
            .await
            .unwrap();
        assert_eq!(attempts(&store), 2, "{change:?}");
        let saved = h.stored(&agent.id).await;
        assert_eq!(saved.work.step, AgentWorkStep::AddWorktree);
        let tree = h
            .store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap();
        let workspace = h
            .store
            .get_workspace(&agent.workspace_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(tree.base_commit), workspace.resolved_commit);
        assert_eq!(
            h.backend.requests().await.len(),
            before,
            "binding must not execute AddWorktree"
        );
        preserves_metadata(h, &agent.id, change).await;
        h.run(&agent.id, h.now).await;
        assert_eq!(calls(h, WireOperation::AddWorktree).await, 1);
        assert_eq!(
            h.stored(&agent.id).await.work.step,
            AgentWorkStep::MaterializeHome
        );
    }
}

#[tokio::test(start_paused = true)]
async fn wait_defer_retries_without_failure_log_or_native_effect() {
    for change in [Change::None, Change::Title, Change::Log] {
        let mut fixture = Waiting::new().await;
        let agent = fixture.admit().await;
        let h = &mut fixture.h;
        let claimed = claim(h, &agent.id).await;
        let before = h.backend.requests().await.len();
        let store = inject(
            h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Any,
            Fault::Abort(change),
        );
        let started = tokio::time::Instant::now();
        h.worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await
            .unwrap();
        assert_eq!(attempts(&store), 2, "{change:?}");
        assert!(started.elapsed() >= Duration::from_secs(10));
        let saved = h.stored(&agent.id).await;
        assert_eq!(saved.status, AgentStatus::Starting);
        assert_eq!(saved.work.step, AgentWorkStep::WaitForWorkspace);
        assert_eq!(saved.work.attempts, claimed.work.attempts + 1);
        assert!(saved.work.lease.is_none());
        assert!(saved.failure.is_none());
        assert_eq!(h.backend.requests().await.len(), before);
        let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert!(!logs
            .iter()
            .any(|entry| matches!(entry.kind, AgentLogKind::WorkStepFailed { .. })));
        preserves_metadata(h, &agent.id, change).await;
    }
}

#[tokio::test(start_paused = true)]
async fn wait_bind_refuses_changed_claim_intent_or_parent() {
    for &change in INCOMPATIBLE {
        let mut fixture = Waiting::new().await;
        let agent = fixture.admit().await;
        fixture.ready().await;
        let h = &mut fixture.h;
        let claimed = claim(h, &agent.id).await;
        let store = inject(
            h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Any,
            Fault::Abort(change),
        );
        let result = h.worker.execute_claimed_for_test(claimed, h.now).await;
        assert!(
            matches!(result, Err(SandboxStoreError::Conflict)),
            "{change:?}: {result:?}"
        );
        assert_eq!(attempts(&store), 1, "{change:?}");
        assert_eq!(calls(h, WireOperation::AddWorktree).await, 0);
    }
}

#[tokio::test(start_paused = true)]
async fn wait_bind_and_defer_do_not_retry_unavailable_publication() {
    for ready in [false, true] {
        let mut fixture = Waiting::new().await;
        let agent = fixture.admit().await;
        if ready {
            fixture.ready().await;
        }
        let h = &mut fixture.h;
        let claimed = claim(h, &agent.id).await;
        let store = inject(
            h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Any,
            Fault::Unavailable,
        );
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(_))),
            "ready={ready}: {result:?}"
        );
        assert_eq!(attempts(&store), 1);
        assert_eq!(h.stored(&agent.id).await, claimed);
        assert_eq!(calls(h, WireOperation::AddWorktree).await, 0);
    }
}
