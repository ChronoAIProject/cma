use super::*;
use crate::sandbox::agent_worker::WorkerFailure;

#[tokio::test]
async fn retryable_failure_rebases_only_publication_after_abort() {
    for change in [Change::None, Change::Title, Change::Log] {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::now();
        let agent = h.admit("failure-publication", vec![]).await;
        let claimed = claim(&h, &agent.id).await;
        let before = h.backend.requests().await.len();
        let store = inject(
            &mut h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Failure,
            Fault::Abort(change),
        );
        h.worker
            .handle_failure(
                claimed.clone(),
                WorkerFailure::retryable("provider_unavailable", "retry observation"),
                h.now,
            )
            .await
            .unwrap();
        let saved = h.stored(&agent.id).await;
        assert_eq!(attempts(&store), 2, "{change:?}");
        assert_eq!(saved.status, AgentStatus::Starting);
        assert_eq!(saved.work.step, claimed.work.step);
        assert_eq!(saved.work.attempts, claimed.work.attempts + 1);
        assert!(saved.work.lease.is_none());
        assert_eq!(saved.work.receipts, claimed.work.receipts);
        assert_eq!(h.backend.requests().await.len(), before);
        let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert_eq!(logs.iter().filter(|entry| matches!(&entry.kind, AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "provider_unavailable")).count(), 1);
        preserves_metadata(&h, &agent.id, change).await;
    }
}

#[tokio::test]
async fn terminal_open_failure_settles_once_after_publication_abort() {
    for change in [Change::None, Change::Title, Change::Log] {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::now();
        let agent = h
            .admit_with_first_message("terminal-publication", vec![], Some("Keep original prompt"))
            .await;
        h.run(&agent.id, h.now).await;
        h.run(&agent.id, h.now).await;
        let claimed = claim(&h, &agent.id).await;
        assert_eq!(claimed.work.step, AgentWorkStep::OpenConversation);
        h.backend.fail_next(
            WireOperation::OpenAgent,
            "request_rejected",
            "terminal refusal",
        );
        let store = inject(
            &mut h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Failure,
            Fault::Abort(change),
        );
        h.worker
            .execute_claimed_for_test(claimed.clone(), h.now)
            .await
            .unwrap();
        let failed = h.stored(&agent.id).await;
        assert_eq!(attempts(&store), 2, "{change:?}");
        assert_eq!(failed.status, AgentStatus::Failed);
        assert_eq!(failed.work.step, AgentWorkStep::None);
        assert!(failed.work.lease.is_none());
        let first = failed.conversation.first_message.as_ref().unwrap();
        assert_eq!(first.state, FirstMessageDelivery::Failed);
        assert_eq!(first.attempt, 0);
        assert_eq!(
            first.payload_sha256,
            claimed
                .conversation
                .first_message
                .as_ref()
                .unwrap()
                .payload_sha256
        );
        assert_eq!(calls(&h, WireOperation::OpenAgent).await, 1);
        assert_eq!(calls(&h, WireOperation::AddWorktree).await, 1);
        preserves_metadata(&h, &agent.id, change).await;
    }
}

#[tokio::test]
async fn terminal_add_failure_publishes_failed_worktree_without_repeating_add() {
    let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.now = SystemTime::now();
    let agent = h.admit("failed-add-publication", vec![]).await;
    let claimed = claim(&h, &agent.id).await;
    h.backend.fail_next(
        WireOperation::AddWorktree,
        "request_rejected",
        "terminal refusal",
    );
    let store = inject(
        &mut h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Failure,
        Fault::Abort(Change::Title),
    );
    h.worker
        .execute_claimed_for_test(claimed, h.now)
        .await
        .unwrap();
    assert_eq!(attempts(&store), 2);
    assert_eq!(h.stored(&agent.id).await.status, AgentStatus::Failed);
    let tree = h
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tree.status, WorktreeStatus::Failed);
    assert_eq!(calls(&h, WireOperation::AddWorktree).await, 1);
    preserves_metadata(&h, &agent.id, Change::Title).await;
}

#[tokio::test]
async fn failure_publication_refuses_changed_intent_claim_or_parent() {
    for &change in INCOMPATIBLE {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::now();
        let agent = h.admit("failure-refusal", vec![]).await;
        let claimed = claim(&h, &agent.id).await;
        let store = inject(
            &mut h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Failure,
            Fault::Abort(change),
        );
        let result = h
            .worker
            .handle_failure(
                claimed,
                WorkerFailure::retryable("provider_unavailable", "retry observation"),
                h.now,
            )
            .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Conflict)),
            "{change:?}: {result:?}"
        );
        assert_eq!(attempts(&store), 1, "{change:?}");
        let logs = h.store.agent_log(&agent.id, 0, 100).await.unwrap();
        assert!(
            !logs
                .iter()
                .any(|entry| matches!(entry.kind, AgentLogKind::WorkStepFailed { .. })),
            "{change:?}"
        );
    }
}

#[tokio::test]
async fn failure_publication_abort_budget_is_bounded() {
    let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.now = SystemTime::now();
    let agent = h.admit("failure-bounded", vec![]).await;
    let claimed = claim(&h, &agent.id).await;
    let store = inject(
        &mut h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Failure,
        Fault::AlwaysAbort,
    );
    let result = h
        .worker
        .handle_failure(
            claimed.clone(),
            WorkerFailure::retryable("provider_unavailable", "retry observation"),
            h.now,
        )
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert!(
        (2..=8).contains(&attempts(&store)),
        "bounded publication attempts: {}",
        attempts(&store)
    );
    assert_eq!(h.stored(&agent.id).await, claimed);
}

#[tokio::test]
async fn failure_publication_does_not_retry_uncertain_commit() {
    for fault in [Fault::Unavailable, Fault::CommittedUnavailable] {
        let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        h.now = SystemTime::now();
        let agent = h.admit("failure-unknown", vec![]).await;
        let claimed = claim(&h, &agent.id).await;
        let store = inject(
            &mut h,
            Arc::new(EmptyAgentSkillResolver),
            Boundary::Failure,
            fault,
        );
        let result = h
            .worker
            .handle_failure(
                claimed.clone(),
                WorkerFailure::retryable("provider_unavailable", "retry observation"),
                h.now,
            )
            .await;
        assert!(
            matches!(result, Err(SandboxStoreError::Unavailable(_))),
            "{fault:?}: {result:?}"
        );
        assert_eq!(attempts(&store), 1);
        let saved = h.stored(&agent.id).await;
        if matches!(fault, Fault::Unavailable) {
            assert_eq!(saved, claimed);
        } else {
            assert_eq!(saved.work.attempts, claimed.work.attempts + 1);
            assert_eq!(
                saved.conversation.last_seq,
                claimed.conversation.last_seq + 1
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn failure_publication_stops_when_claim_expires_during_abort() {
    let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.now = SystemTime::now();
    let agent = h.admit("failure-expired", vec![]).await;
    let claimed = claim(&h, &agent.id).await;
    let store = inject(
        &mut h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Failure,
        Fault::Abort(Change::Expire),
    );
    let result = h
        .worker
        .handle_failure(
            claimed.clone(),
            WorkerFailure::retryable("provider_unavailable", "retry observation"),
            h.now,
        )
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert_eq!(attempts(&store), 1);
    assert_eq!(h.stored(&agent.id).await, claimed);
}

#[tokio::test]
async fn failure_publication_cannot_continue_under_revoked_owner_authority() {
    use layer2_core::agent_ownership::{
        test_support::MemoryOwnershipStore, Member, OwnershipCoordinator, OwnershipError,
        PROTOCOL_VERSION,
    };
    let mut h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    h.now = SystemTime::now();
    let agent = h.admit("failure-owner-revoked", vec![]).await;
    let claimed = claim(&h, &agent.id).await;
    let coordinator = OwnershipCoordinator::register(
        Arc::new(MemoryOwnershipStore::default()),
        Member {
            instance_id: "AM-a".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
    )
    .await
    .unwrap();
    let authority = coordinator.acquire(&agent.id).await.unwrap().unwrap();
    let store = inject(
        &mut h,
        Arc::new(EmptyAgentSkillResolver),
        Boundary::Failure,
        Fault::Abort(Change::RevokeAuthority),
    );
    let result = authority
        .run(h.worker.handle_failure(
            claimed.clone(),
            WorkerFailure::retryable("provider_unavailable", "retry observation"),
            h.now,
        ))
        .await;
    assert!(matches!(result, Err(OwnershipError::Stale)));
    assert_eq!(attempts(&store), 1);
    assert_eq!(h.stored(&agent.id).await, claimed);
}
