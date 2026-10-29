use super::*;
use layer2_core::{Authorization, Caller, Capability};

#[tokio::test]
async fn worktree_recovery_keeps_original_failure_and_completes_same_agent_startup() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let original = harness.admit("worktree-recovery", vec![]).await;
    harness.backend.fail_next(
        WireOperation::AddWorktree,
        "runtime_mutation_outcome_unknown",
        "worktree addition left partial or mismatched Git state; explicit recovery is required",
    );
    harness.run(&original.id, harness.now).await;
    let failed = harness.stored(&original.id).await;
    assert_eq!(failed.status, AgentStatus::Failed);
    assert_eq!(
        failed.failure.as_ref().unwrap().class,
        "agent_runtime_rejected"
    );
    let original_worktree = harness
        .store
        .get_worktree(&failed.worktree_id)
        .await
        .unwrap()
        .unwrap();
    let old_logs = harness.store.agent_log(&failed.id, 0, 100).await.unwrap();
    let owner = Caller::new(
        failed.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let next = harness
        .store
        .begin_agent_worktree_recovery(
            &failed.id,
            &owner,
            "awr_explicit",
            &WorktreeId::generate(),
            rfc3339_at(harness.now),
        )
        .await
        .unwrap();
    assert_eq!(next.id, original.id);
    assert_ne!(next.worktree_id, original.worktree_id);
    assert_eq!(
        next.work.worktree_recoveries[0].original_worktree,
        original_worktree
    );
    let ready = harness.drive_ready(&original.id).await;
    assert!(ready.conversation.conversation_id.is_some());
    assert_eq!(
        ready.work.worktree_recoveries,
        next.work.worktree_recoveries
    );
    let retained = harness
        .store
        .get_worktree(&original.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, WorktreeStatus::Failed);
    assert_eq!(retained.path, original_worktree.path);
    assert_eq!(retained.branch, original_worktree.branch);
    let logs = harness.store.agent_log(&ready.id, 0, 100).await.unwrap();
    assert_eq!(&logs[..old_logs.len()], &old_logs);
    let requests = harness.backend.requests().await;
    let additions = requests
        .iter()
        .filter_map(|(_, request)| match request {
            layer3_adapter::wire::RequestFrame::AddWorktree(value) => {
                Some(value.request.name.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(additions, ["wt-1", "wt-2"]);
    assert!(!requests.iter().any(|(_, request)| matches!(
        request,
        layer3_adapter::wire::RequestFrame::RemoveWorktree(_)
    )));
}
