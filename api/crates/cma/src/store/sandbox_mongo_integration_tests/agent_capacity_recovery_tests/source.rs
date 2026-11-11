use super::*;
use crate::store::agent_capacity_recovery::source::{capture_segmented, fence_agent};
use crate::store::agent_capacity_recovery::source::{
    capture_segmented_scoped, fence_filter_scoped,
};
use crate::store::agent_log_segments::transaction_scope::TransactionScope;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_scoped_fence_is_atomic_and_rejects_stale_aggregate() {
    let (client, name) = database("capacity_scoped_fence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "scoped").await;
    let before = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(capture_segmented_scoped(&store, &agent, &mut scope)
        .await
        .unwrap()
        .is_none());
    let filter = fence_filter_scoped(&store, &agent, &mut scope)
        .await
        .unwrap();
    assert_eq!(
        scope
            .update_one(
                &store.agents,
                filter,
                doc! {"$inc": {"checkpoint_capture_fence": 1_i64}}
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        before,
        store
            .agents
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap()
    );
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    let mut stale = agent.clone();
    stale.revision += 1;
    assert!(fence_filter_scoped(&store, &stale, &mut scope)
        .await
        .is_err());
    scope.release().abort_transaction().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

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
