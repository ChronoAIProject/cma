use super::*;
use layer2_core::sandbox::store::SandboxStoreError;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn compatibility_vectors_refuse_total_bounds_while_requested_pages_succeed() {
    let (client, name) = database("compat_bounds").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, rows) = fixtures::source(&store, 1000, false).await;
    assert!(rows.len() > 10_128);
    // The row count fits, but the requested vector exceeds its byte budget.
    let fitting_count = 10_000;
    assert!(
        rows[..fitting_count]
            .iter()
            .map(|row| serde_json::to_vec(row).unwrap().len())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    for through in [fitting_count as u64, agent.conversation.last_seq] {
        assert_eq!(
            store.agent_log_prefix(&agent.id, through).await,
            Err(SandboxStoreError::AgentLogCapacityExceeded)
        );
    }
    assert_eq!(
        store.agent_log_prefix(&agent.id, 400).await.unwrap(),
        rows[..400]
    );
    // Small windows remain available beyond the old lifetime boundary without
    // enabling admission or optional legacy optimization flags.
    let after = agent.conversation.last_seq - 3;
    assert_eq!(
        store
            .agent_log_range(&agent.id, after, agent.conversation.last_seq, 3)
            .await
            .unwrap(),
        rows[after as usize..]
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn empty_compatibility_windows_still_validate_the_original_captured_tail() {
    let (client, name) = database("compat_empty").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, rows) = segmented(&store, 2).await;
    let head = agent.conversation.last_seq;
    for cursor in [0, head, head + 1, u64::MAX] {
        assert!(store
            .agent_log_range(&agent.id, cursor, cursor, 1)
            .await
            .unwrap()
            .is_empty());
    }
    assert!(store
        .agent_log_prefix(&agent.id, 0)
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .agent_log(&agent.id, u64::MAX, 1)
        .await
        .unwrap()
        .is_empty());

    let extra = AgentLogEntry {
        seq: head + 1,
        ..rows.last().unwrap().clone()
    };
    store
        .agent_log
        .insert_one(log_document(&extra).unwrap())
        .await
        .unwrap();
    for cursor in [0, head, head + 1, u64::MAX] {
        assert_eq!(
            store.agent_log_range(&agent.id, cursor, cursor, 1).await,
            Err(SandboxStoreError::SequenceConflict)
        );
    }
    assert_eq!(
        store.agent_log_prefix(&agent.id, 0).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    assert_eq!(
        store.agent_log(&agent.id, u64::MAX, 1).await,
        Err(SandboxStoreError::SequenceConflict)
    );
    db.drop().await.unwrap();
}
