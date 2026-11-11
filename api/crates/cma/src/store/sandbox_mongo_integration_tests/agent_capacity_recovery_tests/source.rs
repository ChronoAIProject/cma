use super::*;
use crate::store::agent_capacity_recovery::source::{capture_segmented, fence_agent};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_source_legacy_dispatch_and_fence_keep_record_unchanged() {
    let (client, name) = database("capacity_source_legacy").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "legacy").await;
    let before = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(capture_segmented(&store, &agent, &mut session)
        .await
        .unwrap()
        .is_none());
    fence_agent(&store, &agent, "checkpoint_capture_fence", &mut session)
        .await
        .unwrap();
    session.commit_transaction().await.unwrap();
    let after = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.get("record"), after.get("record"));
    assert_eq!(after.get_i64("checkpoint_capture_fence").unwrap(), 1);
    let mut changed = agent.clone();
    changed.revision += 1;
    start_transaction(&mut session).await.unwrap();
    assert!(capture_segmented(&store, &changed, &mut session)
        .await
        .is_err());
    session.abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_source_malformed_envelope_never_selects_legacy_fallback() {
    let (client, name) = database("capacity_source_corrupt").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "corrupt").await;
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"agent_log_format": "segmented_v1"}},
        )
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert!(capture_segmented(&store, &agent, &mut session)
        .await
        .is_err());
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! {"agent_id": agent.id.as_str()})
            .await
            .unwrap(),
        agent.conversation.last_seq
    );
    client.database(&name).drop().await.unwrap();
}
