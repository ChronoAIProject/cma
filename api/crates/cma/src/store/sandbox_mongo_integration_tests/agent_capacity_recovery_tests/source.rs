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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_lineage_refuses_oversized_child_set_before_mutation() {
    let (client, name) = database("capacity_lineage_bound").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = ready_conversation_fixture(&store, "bounded").await;
    let mut rows = Vec::new();
    for index in 0..512 {
        let mut agent = original.clone();
        agent.id = AgentId(format!("bounded-child-{index:04}"));
        agent.creation_key = format!("bounded-creation-{index:04}");
        agent.worktree_id = WorktreeId(format!("bounded-worktree-{index:04}"));
        rows.push(crate::store::agents::agent_document(&agent).unwrap());
    }
    store.agents.insert_many(rows).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(matches!(
        crate::store::agent_capacity_recovery::lineage::agents(
            &store,
            &original.sandbox_id,
            &mut scope
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(store.agents.count_documents(doc! {}).await.unwrap(), 513);
    assert_eq!(store.get_agent(&original.id).await.unwrap(), Some(original));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn capacity_lineage_refuses_oversized_workspace_set_before_mutation() {
    let (client, name) = database("capacity_workspace_bound").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "workspace-bound").await;
    let original = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let mut rows = Vec::new();
    for index in 0..512 {
        let mut workspace = original.clone();
        workspace.id = WorkspaceId(format!("bounded-workspace-{index:04}"));
        workspace.name = format!("bounded-workspace-{index:04}");
        workspace.path = format!("/home/cma/{}", workspace.name);
        rows.push(crate::store::workspaces::workspace_document(&workspace).unwrap());
    }
    store.workspaces.insert_many(rows).await.unwrap();
    let mut session = client.start_session().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert!(matches!(
        crate::store::agent_capacity_recovery::lineage::workspaces(
            &store,
            &agent.sandbox_id,
            &mut scope
        )
        .await,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                SandboxStoreError::Unavailable(_)
            )
        )
    ));
    scope.release().abort_transaction().await.unwrap();
    assert_eq!(
        store.workspaces.count_documents(doc! {}).await.unwrap(),
        513
    );
    assert_eq!(
        store.get_workspace(&original.id).await.unwrap(),
        Some(original)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}
