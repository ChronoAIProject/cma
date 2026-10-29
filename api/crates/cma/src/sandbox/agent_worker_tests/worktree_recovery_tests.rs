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

#[tokio::test]
async fn published_agent_stop_does_not_wait_for_old_connection_lease_and_fences_old_publication() {
    use layer2_core::sandbox::store::{AgentStopRequest, AgentWorkMutation, SandboxStoreError};
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let agent = harness.admit("stop-published-lease", vec![]).await;
    let mut ready = harness.drive_ready(&agent.id).await;
    let expected = ready.revision;
    ready
        .work
        .schedule(AgentWorkStep::PublishConnection, rfc3339_at(harness.now));
    ready.revision += 1;
    harness
        .store
        .save_agent(expected, ready.clone())
        .await
        .unwrap();
    let stale = harness
        .store
        .claim_agent_work(
            &agent.id,
            "old-connection-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(18 * 60)),
        )
        .await
        .unwrap()
        .unwrap();
    let stop_at = harness.now + Duration::from_secs(1);
    let stopped = harness
        .store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&stale),
            reason: AgentStopReason::BindingRetired,
            requested_at: rfc3339_at(stop_at),
        })
        .await
        .unwrap()
        .into_agent();
    assert!(stopped.work.cleanup_not_before.is_none());
    let old_fence = stale.work.lease.as_ref().unwrap().fence;
    let mut late = stale.clone();
    late.work
        .advance(
            "old-connection-worker",
            old_fence,
            AgentWorkStep::None,
            rfc3339_at(stop_at),
        )
        .unwrap();
    late.revision += 1;
    assert_eq!(
        harness
            .store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: stale.revision,
                holder: "old-connection-worker".into(),
                fence: old_fence,
                agent: late,
                worktree: None,
                appended_at: rfc3339_at(stop_at),
                sandbox_event: None,
                agent_logs: vec![],
            })
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    for _ in 0..3 {
        harness.run(&agent.id, stop_at).await;
    }
    let terminal = harness.stored(&agent.id).await;
    assert_eq!(terminal.status, AgentStatus::Stopped);
    assert_eq!(terminal.stop_reason, Some(AgentStopReason::BindingRetired));
    assert_eq!(
        terminal.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert_eq!(terminal.worktree_id, ready.worktree_id);
    assert!(harness
        .backend
        .snapshot(&harness.runtime)
        .await
        .unwrap()
        .homes
        .contains_key(&AgentSlot::new(agent.id.to_string())));
}
