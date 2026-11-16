use super::*;
use layer2_core::sandbox::store::AgentWorkMutation;
use layer2_core::sandbox::AgentCleanupDebt;
use layer3_adapter::WorktreeRequest;

#[tokio::test]
async fn unchanged_agent_after_aborted_publication_can_commit_worktree_intent() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let (current, parent) = claimed_startup(&harness).await;
    let tree = harness
        .store
        .get_worktree(&current.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let request = WorktreeRequest::new(
        format!("worktree:{}:{}:add:v1", harness.workspace.name, tree.name),
        harness.workspace.name.clone(),
        tree.name,
        tree.base_commit,
    );
    let debt = AgentCleanupDebt {
        step: AgentWorkStep::AddWorktree,
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
    };
    let calls = AtomicUsize::new(0);
    harness
        .worker
        .retry_startup_publication(current.clone(), &parent, harness.now, |mut agent, at| {
            let attempt = calls.fetch_add(1, Ordering::SeqCst);
            let store = harness.store.clone();
            let debt = debt.clone();
            async move {
                // Model a definitively aborted fence write, with no Agent change.
                if attempt == 0 {
                    return Err(SandboxStoreError::Conflict);
                }
                let expected = agent.revision;
                let fence = agent.work.lease.as_ref().unwrap().fence;
                agent.work.cleanup_debt.push(debt);
                agent.revision += 1;
                agent.updated_at = rfc3339_at(at);
                store
                    .commit_agent_work(AgentWorkMutation {
                        expected_agent_revision: expected,
                        holder: "agent-worker".into(),
                        fence,
                        agent,
                        worktree: None,
                        appended_at: rfc3339_at(at),
                        sandbox_event: None,
                        agent_logs: vec![],
                    })
                    .await
            }
        })
        .await
        .unwrap();
    let after = harness.stored(&current.id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(after.revision, current.revision + 1);
    assert_eq!(after.work.cleanup_debt, vec![debt]);
    assert_eq!(after.work.lease, current.work.lease);
    assert_eq!(after.work.step, AgentWorkStep::AddWorktree);
    assert!(after.work.receipts.is_empty());
}

#[tokio::test]
async fn unchanged_agent_conflicts_remain_bounded_without_releasing_claim() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let (current, parent) = claimed_startup(&harness).await;
    let calls = AtomicUsize::new(0);
    let result = harness
        .worker
        .retry_startup_publication(current.clone(), &parent, harness.now, |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err::<(), _>(SandboxStoreError::Conflict))
        })
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(calls.load(Ordering::SeqCst), 8);
    assert_eq!(harness.stored(&current.id).await, current);
}

#[tokio::test]
async fn unchanged_agent_conflict_still_checks_parent_retirement() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let (current, parent) = claimed_startup(&harness).await;
    let calls = AtomicUsize::new(0);
    let result = harness
        .worker
        .retry_startup_publication(current.clone(), &parent, harness.now, |_, at| {
            calls.fetch_add(1, Ordering::SeqCst);
            let store = harness.store.clone();
            let mut retired = parent.clone();
            async move {
                let expected = retired.revision;
                retired.begin_retirement(rfc3339_at(at)).unwrap();
                store.save_sandbox(expected, retired).await.unwrap();
                Err::<(), _>(SandboxStoreError::Conflict)
            }
        })
        .await;
    assert_eq!(result, Err(SandboxStoreError::Conflict));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(harness.stored(&current.id).await, current);
}
