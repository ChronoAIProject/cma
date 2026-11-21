use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_invalid_worktree_rolls_back_every_artifact() {
    let (client, name) = database("agent_work_worktree_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, "worktree_rollback").await;
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 1;
    let mut invalid_worktree = worktree.clone();
    invalid_worktree.status = WorktreeStatus::Ready;
    invalid_worktree.revision += 2;

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: worktree.revision,
                    worktree: invalid_worktree,
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_late_log_conflict_rolls_back_agent_worktree_and_earlier_log() {
    let (client, name) = database("agent_work_log_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "log_rollback").await;
    let conflicting = AgentLogEntry {
        agent_id: claimed.id.clone(),
        seq: 2,
        appended_at: NOW.into(),
        kind: AgentLogKind::Stopped {
            reason: "conflict".into(),
        },
    };
    store
        .agent_log
        .insert_one(log_document(&conflicting).unwrap())
        .await
        .unwrap();
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 2;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert!(matches!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted, AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert!(store
        .agent_log
        .find_one(doc! { "agent_id": conflicting.agent_id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_rejects_noncolliding_orphan_log_before_publication() {
    let (client, name) = database("agent_work_orphan_log").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) = claimed_agent_work_fixture(&store, "orphan_log").await;
    let orphan = AgentLogEntry {
        agent_id: claimed.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::Deleted,
    };
    store
        .agent_log
        .insert_one(log_document(&orphan).unwrap())
        .await
        .unwrap();
    let mut next = advance_claimed_agent(&claimed);
    next.conversation.last_seq += 1;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert!(matches!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence: claimed.work.lease.as_ref().unwrap().fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::Created),
                agent_logs: vec![AgentLogKind::Deleted],
            })
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    assert!(store
        .agent_log
        .find_one(doc! { "agent_id": orphan.agent_id.as_str(), "seq": 1_i64 })
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        0
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_work_late_event_conflict_rolls_back_agent_worktree_and_logs() {
    let (client, name) = database("agent_work_event_rollback").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut worktree, claimed) =
        claimed_agent_work_fixture(&store, "event_rollback").await;
    let claimed = ready_publication_claim(claimed, "conversation-event-rollback");
    replace_agent_fixture(&store, &claimed).await;
    let conflicting = SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    store
        .sandbox_events
        .insert_one(event_document(&conflicting).unwrap())
        .await
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance(
            "worker",
            fence,
            AgentWorkStep::PublishConnection,
            NOW.into(),
        )
        .unwrap();
    next.transition(AgentStatus::Ready, NOW.into(), None)
        .unwrap();
    next.conversation.last_seq += 2;
    let expected_worktree_revision = worktree.revision;
    worktree.transition(WorktreeStatus::Ready).unwrap();

    assert_eq!(
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "worker".into(),
                fence,
                agent: next,
                worktree: Some(VersionedWorktree {
                    expected_revision: expected_worktree_revision,
                    worktree: worktree.clone(),
                }),
                appended_at: NOW.into(),
                sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: claimed.id.clone(),
                    from: AgentStatus::Starting,
                    to: AgentStatus::Ready,
                }),
                agent_logs: vec![
                    AgentLogKind::ConversationOpened {
                        conversation_id: "conversation-event-rollback".into(),
                        runtime_generation: 1,
                    },
                    AgentLogKind::StatusChanged {
                        from: AgentStatus::Starting,
                        to: AgentStatus::Ready,
                    },
                ],
            })
            .await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(store.get_agent(&claimed.id).await.unwrap(), Some(claimed));
    let mut original_worktree = worktree;
    original_worktree.status = WorktreeStatus::Creating;
    original_worktree.revision = expected_worktree_revision;
    assert_eq!(
        store.get_worktree(&original_worktree.id).await.unwrap(),
        Some(original_worktree)
    );
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        store.sandbox_events.count_documents(doc! {}).await.unwrap(),
        1
    );
    let sandbox_row = store
        .sandboxes
        .find_one(doc! { "_id": parent.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_row.get_i64("last_seq").unwrap(), 0);

    client.database(&name).drop().await.unwrap();
}
