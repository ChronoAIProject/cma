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

#[tokio::test(start_paused = true)]
async fn wait_bind_rebuilds_worktree_cas_after_revision_only_abort() {
    let mut fixture = Waiting::new().await;
    let agent = fixture.admit().await;
    fixture.ready().await;
    let h = &mut fixture.h;
    let claimed = claim(h, &agent.id).await;
    let original_tree = h
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert!(original_tree.base_commit.is_empty());
    let before = h.backend.requests().await.len();
    let store = inject(
        h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Any,
        Fault::Abort(Change::WorktreeRevision),
    );
    h.worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await
        .unwrap();
    assert_eq!(attempts(&store), 2);
    let saved = h.stored(&agent.id).await;
    assert_eq!(saved.work.step, AgentWorkStep::AddWorktree);
    assert!(saved.work.lease.is_none());
    assert_eq!(saved.work.receipts, claimed.work.receipts);
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
    assert_eq!(tree.revision, original_tree.revision + 2);
    assert_eq!(Some(tree.base_commit.clone()), workspace.resolved_commit);
    let mut expected_tree = original_tree;
    expected_tree.revision += 2;
    expected_tree.base_commit = tree.base_commit.clone();
    assert_eq!(tree, expected_tree);
    let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(
                entry.kind,
                AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::WaitForWorkspace
                }
            ))
            .count(),
        1
    );
    assert_eq!(
        saved.conversation.last_seq,
        claimed.conversation.last_seq + 1
    );
    assert_eq!(h.backend.requests().await.len(), before);
}

#[tokio::test(start_paused = true)]
async fn wait_keeps_progress_when_claimed_agent_title_changed_before_loop() {
    let mut fixture = Waiting::new().await;
    let agent = fixture.admit().await;
    fixture.ready().await;
    let h = &mut fixture.h;
    let claimed = claim(h, &agent.id).await;
    let mut renamed = claimed.clone();
    renamed.title = Some("Concurrent owner title".into());
    renamed.revision += 1;
    h.store
        .save_agent(claimed.revision, renamed.clone())
        .await
        .unwrap();
    let before = h.backend.requests().await.len();
    let store = inject(
        h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Any,
        Fault::Abort(Change::None),
    );
    // The worker deliberately receives its old claim snapshot. The current
    // store differs only by compatible display metadata before its first read.
    h.worker
        .execute_claimed_for_test(claimed.clone(), h.now)
        .await
        .unwrap();
    assert_eq!(attempts(&store), 2);
    let saved = h.stored(&agent.id).await;
    assert_eq!(saved.work.step, AgentWorkStep::AddWorktree);
    assert!(saved.work.lease.is_none());
    assert_eq!(saved.title, renamed.title);
    assert_eq!(saved.revision, renamed.revision + 1);
    assert_eq!(
        saved.conversation.last_seq,
        claimed.conversation.last_seq + 1
    );
    assert_eq!(h.backend.requests().await.len(), before);
    preserves_metadata(h, &agent.id, Change::Title).await;
}
