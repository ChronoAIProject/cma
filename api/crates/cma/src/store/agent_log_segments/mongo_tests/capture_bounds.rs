use super::capture_tests::legacy;
use super::*;
use segments::capture::{capture, points, CaptureMode};

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn legacy_capture_certifies_independent_opened_and_retired_maxima() {
    let (client, name) = database("capture_generations").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (mut agent, entries) = legacy(
        &store,
        vec![
            AgentLogKind::ConversationOpened {
                conversation_id: "native-fixture".into(),
                runtime_generation: 2,
            },
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation: 2,
                runtime_generation: 3,
            },
            AgentLogKind::ConversationOpened {
                conversation_id: "next-native-fixture".into(),
                runtime_generation: 4,
            },
        ],
    )
    .await;
    agent.conversation.conversation_id = Some("next-native-fixture".into());
    agent.conversation.runtime_generation = Some(4);
    store
        .agents
        .replace_one(
            doc! {"_id":agent.id.as_str()},
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .unwrap();
    assert_eq!(captured.max_opened_generation(), 4);
    assert_eq!(captured.max_retired_generation(), 3);
    assert_eq!(captured.latest_entry(), entries.last());
    assert_eq!(
        points(
            &store,
            &captured,
            &[2, 3],
            &mut session,
            CaptureMode::Snapshot
        )
        .await
        .unwrap(),
        entries[1..3]
    );
    drop(session);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn capture_proves_empty_history_and_refuses_extra_or_over_capacity_source() {
    let (client, name) = database("capture_empty").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = agent();
    store
        .agents
        .insert_one(crate::store::agents::agent_document(&agent).unwrap())
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let captured = capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .unwrap();
    assert_eq!(captured.through(), 0);
    assert!(captured.latest_entry().is_none());
    assert!(captured.active_leaf_digests().bytes.is_empty());
    assert_eq!(captured.active_head().active_bytes, 0);
    assert!(
        points(&store, &captured, &[1], &mut session, CaptureMode::Snapshot)
            .await
            .is_err()
    );
    drop(session);
    let row = log_document(&AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 1,
        appended_at: NOW.into(),
        kind: AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
    })
    .unwrap();
    store.agent_log.insert_one(row).await.unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(capture(&store, &agent, &mut session, CaptureMode::Snapshot)
        .await
        .is_err());
    drop(session);
    let mut too_large = agent;
    too_large.conversation.last_seq = MAX_ENTRIES as u64 + 1;
    store
        .agents
        .replace_one(
            doc! {"_id":too_large.id.as_str()},
            crate::store::agents::agent_document(&too_large).unwrap(),
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    crate::store::mongo_read_telemetry::isolated_test(async {
        assert!(matches!(
            capture(&store, &too_large, &mut session, CaptureMode::Snapshot).await,
            Err(
                crate::store::agent_log_integrity::TransactionLogError::Domain(
                    SandboxStoreError::AgentLogCapacityExceeded
                )
            )
        ));
        assert_eq!(
            crate::store::mongo_read_telemetry::snapshot("agent_log"),
            [0; 4]
        );
    })
    .await;
    assert_eq!(store.agent_log.count_documents(doc! {}).await.unwrap(), 1);
    drop(session);
    client.database(&name).drop().await.unwrap();
}
