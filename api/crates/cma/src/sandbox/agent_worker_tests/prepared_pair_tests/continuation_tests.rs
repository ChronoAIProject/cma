use super::*;
use layer2_core::sandbox::SandboxStoreError;

pub(super) enum ReplayChange {
    SandboxRevision,
    WorkspaceRevision,
    WorktreeRevision,
    Runtime,
    Pause,
    Holder,
    Expire,
    FirstMessage,
}

impl ReplayChange {
    pub(super) async fn apply(self, adapter: &ObservedAdapter) {
        let mut agent = adapter
            .store
            .get_agent(&adapter.agent_id)
            .await
            .unwrap()
            .unwrap();
        match self {
            Self::SandboxRevision | Self::Runtime | Self::Pause => {
                let mut parent = adapter
                    .store
                    .get_sandbox(&agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                match self {
                    Self::Runtime => {
                        parent.runtime.as_mut().unwrap().runtime_id = "changed-runtime".into();
                        parent.revision += 1;
                    }
                    Self::Pause => parent.begin_pause(agent.updated_at.clone(), None).unwrap(),
                    _ => parent.revision += 1,
                }
                adapter.store.save_sandbox(revision, parent).await.unwrap();
            }
            Self::WorkspaceRevision => {
                let mut parent = adapter
                    .store
                    .get_workspace(&agent.workspace_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                parent.revision += 1;
                adapter
                    .store
                    .save_workspace(revision, parent)
                    .await
                    .unwrap();
            }
            Self::WorktreeRevision => {
                let mut parent = adapter
                    .store
                    .get_worktree(&agent.worktree_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                parent.revision += 1;
                adapter.store.save_worktree(revision, parent).await.unwrap();
            }
            Self::Holder | Self::FirstMessage => {
                let revision = agent.revision;
                if matches!(self, Self::Holder) {
                    agent.work.lease.as_mut().unwrap().holder = "successor".into();
                } else {
                    agent.title = Some("Owner title during replay".into());
                    agent.conversation.first_message = Some(
                        FirstMessageRecord::new(
                            &agent.creation_key,
                            "Execute once",
                            &agent.updated_at,
                        )
                        .unwrap(),
                    );
                }
                agent.revision += 1;
                adapter.store.save_agent(revision, agent).await.unwrap();
            }
            Self::Expire => tokio::time::sleep(Duration::from_secs(30)).await,
        }
    }
}

#[tokio::test]
async fn prepared_continuation_claims_open_and_preserves_native_conversation() {
    for supported in [false, true] {
        let (mut harness, agent) = prepared().await;
        let adapter = use_adapter(&mut harness, &agent, supported);
        let claimed = claim(&harness, &agent, harness.now).await;
        let replay_fence = claimed.work.lease.as_ref().unwrap().fence;
        harness
            .worker
            .replay_and_open_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        let current = harness.stored(&agent.id).await;
        assert_eq!(adapter.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(adapter.opens.load(Ordering::SeqCst), usize::from(supported));
        assert_eq!(current.work.receipts.len(), if supported { 3 } else { 2 });
        assert_eq!(
            current.work.step,
            if supported {
                AgentWorkStep::PublishReady
            } else {
                AgentWorkStep::OpenConversation
            }
        );
        assert!(current.work.cleanup_debt.is_empty());
        assert!(current.work.lease.is_none());
        if supported {
            assert_eq!(
                current.conversation.conversation_id,
                agent.work.warm_conversation_id
            );
            assert_eq!(current.work.next_fence, replay_fence + 2);
        }
        for _ in 0..if supported { 2 } else { 3 } {
            harness.run(&agent.id, harness.now).await;
        }
        let ready = harness.stored(&agent.id).await;
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(
            ready.conversation.conversation_id,
            agent.work.warm_conversation_id
        );
        assert!(harness
            .registry
            .published_target_is_attached(&Harness::target(&ready)));
        let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
        assert_eq!(
            (
                snapshot.worktrees.len(),
                snapshot.homes.len(),
                snapshot.agents.len()
            ),
            (1, 1, 1)
        );
    }
}

#[tokio::test]
async fn prepared_continuation_reinspects_any_changed_parent_before_open() {
    for change in [
        ReplayChange::SandboxRevision,
        ReplayChange::WorkspaceRevision,
        ReplayChange::WorktreeRevision,
    ] {
        let (mut harness, agent) = prepared().await;
        let adapter = use_adapter(&mut harness, &agent, true);
        *adapter.after_replay.lock().unwrap() = Some(change);
        let claimed = claim(&harness, &agent, harness.now).await;
        harness
            .worker
            .replay_and_open_prepared_for_test(claimed, harness.now)
            .await
            .unwrap();
        assert_eq!(adapter.inspections.load(Ordering::SeqCst), 2);
        assert_eq!(adapter.opens.load(Ordering::SeqCst), 1);
        assert_eq!(
            harness.stored(&agent.id).await.work.step,
            AgentWorkStep::PublishReady
        );
    }
}

#[tokio::test(start_paused = true)]
async fn prepared_continuation_refuses_changed_runtime_parent_or_replay_authority() {
    for change in [
        ReplayChange::Runtime,
        ReplayChange::Pause,
        ReplayChange::Holder,
        ReplayChange::Expire,
    ] {
        let (mut harness, agent) = prepared().await;
        let adapter = use_adapter(&mut harness, &agent, true);
        *adapter.after_replay.lock().unwrap() = Some(change);
        let claimed = claim(&harness, &agent, harness.now).await;
        // A parent transition may reject the paired publication or the next
        // inspection. Neither may permit Open or publish a usable conversation.
        let result = harness
            .worker
            .replay_and_open_prepared_for_test(claimed, harness.now)
            .await;
        assert!(result.is_ok() || matches!(result, Err(SandboxStoreError::Conflict)));
        assert_eq!(adapter.opens.load(Ordering::SeqCst), 0);
        let current = harness.stored(&agent.id).await;
        assert!(current.conversation.conversation_id.is_none());
        assert_ne!(current.status, AgentStatus::Ready);
    }
}

#[tokio::test]
async fn prepared_continuation_lost_resource_reply_never_opens_and_retry_reinspects() {
    let (mut harness, agent) = prepared().await;
    let adapter = use_adapter(&mut harness, &agent, true);
    adapter.lose_response.store(true, Ordering::SeqCst);
    let claimed = claim(&harness, &agent, harness.now).await;
    harness
        .worker
        .replay_and_open_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    assert_eq!(adapter.opens.load(Ordering::SeqCst), 0);
    let pending = harness.stored(&agent.id).await;
    assert!(pending.work.receipts.is_empty());
    assert_eq!(pending.work.cleanup_debt.len(), 2);
    let later = harness.now + Duration::from_secs(2);
    let claimed = claim(&harness, &pending, later).await;
    harness
        .worker
        .replay_and_open_prepared_for_test(claimed, later)
        .await
        .unwrap();
    assert_eq!(adapter.inspections.load(Ordering::SeqCst), 2);
    assert_eq!(adapter.opens.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.stored(&agent.id).await.work.step,
        AgentWorkStep::PublishReady
    );
}

#[tokio::test]
async fn prepared_continuation_retains_open_debt_when_fresh_command_authority_fails() {
    let (mut harness, agent) = prepared().await;
    let adapter = use_adapter(&mut harness, &agent, true);
    adapter.refuse_open.store(true, Ordering::SeqCst);
    let claimed = claim(&harness, &agent, harness.now).await;
    harness
        .worker
        .replay_and_open_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    let pending = harness.stored(&agent.id).await;
    assert_eq!(pending.work.step, AgentWorkStep::OpenConversation);
    assert_eq!(pending.work.receipts.len(), 2);
    assert_eq!(pending.work.cleanup_debt.len(), 1);
    assert!(pending.conversation.conversation_id.is_none());
    harness
        .run(&agent.id, harness.now + Duration::from_secs(2))
        .await;
    assert_eq!(adapter.inspections.load(Ordering::SeqCst), 2);
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.work.step, AgentWorkStep::PublishReady);
    assert_eq!(
        current.conversation.conversation_id,
        agent.work.warm_conversation_id
    );
}

#[tokio::test]
async fn prepared_continuation_preserves_owner_message_arriving_during_replay() {
    let (mut harness, agent) = prepared().await;
    // The real first-message service checks authority against wall-clock time.
    harness.now = SystemTime::now();
    let adapter = use_adapter(&mut harness, &agent, true);
    *adapter.after_replay.lock().unwrap() = Some(ReplayChange::FirstMessage);
    let claimed = claim(&harness, &agent, harness.now).await;
    harness
        .worker
        .replay_and_open_prepared_for_test(claimed, harness.now)
        .await
        .unwrap();
    assert_eq!(adapter.opens.load(Ordering::SeqCst), 1);
    for _ in 0..3 {
        harness.run(&agent.id, harness.now).await;
    }
    let current = harness.stored(&agent.id).await;
    assert_eq!(current.title.as_deref(), Some("Owner title during replay"));
    assert_eq!(
        current.conversation.first_message.unwrap().state,
        FirstMessageDelivery::WriteConfirmed
    );
    let log = harness.store.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        log.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
}
