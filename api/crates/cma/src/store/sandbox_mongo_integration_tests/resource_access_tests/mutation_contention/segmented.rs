use super::*;
use futures_util::TryStreamExt;
use mongodb::bson::RawDocumentBuf;

async fn fixture() -> Fixture {
    let f = Fixture::new(false).await;
    segments::schema::migrate(&f.admin.database(&f.name))
        .await
        .unwrap();
    let agent = f.before.lineage.agent.clone().unwrap();
    let logs = f.store.agent_log(&agent.id, 0, 20).await.unwrap();
    f.store
        .agents
        .delete_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    f.store
        .agent_log
        .delete_many(doc! {"agent_id":agent.id.as_str()})
        .await
        .unwrap();
    segments::mongo_tests::fixtures::install(&f.store, agent, logs, 0, 2).await;
    assert_eq!(
        f.store.resource_access_snapshot(&f.target).await.unwrap(),
        f.before,
        "segmented setup must validate before exercising mutation"
    );
    f.trace.commands.lock().unwrap().clear();
    f
}

async fn raw_rows(f: &Fixture, names: &[&str]) -> Vec<Vec<RawDocumentBuf>> {
    let mut records = Vec::new();
    for name in names {
        records.push(
            f.admin
                .database(&f.name)
                .collection::<RawDocumentBuf>(name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    records
}

const HISTORY: &[&str] = &[
    "agent_log",
    segments::HEADS,
    segments::SEGMENTS,
    segments::PROJECTIONS,
    segments::EVIDENCE,
];

async fn assert_guard(f: &Fixture, count: i64) {
    for rows in [
        &f.store.sandboxes,
        &f.store.workspaces,
        &f.store.worktrees,
        &f.store.agents,
    ] {
        let row = rows.find_one(doc! {}).await.unwrap().unwrap();
        assert_eq!(row.get_i64("resource_share_guard").unwrap(), count);
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_share_replay_preserves_exact_head_and_log_bytes() {
    let f = fixture().await;
    let history = raw_rows(&f, HISTORY).await;
    assert_eq!(history[0].len(), 2, "retained log must be exercised");
    assert_eq!(history[1].len(), 1, "segmented head must be exercised");
    let request = mutation(&f.before, "segmented-byte-proof", ShareMode::View);
    let receipt = f.store.mutate_share(request.clone()).await.unwrap();
    assert_eq!(f.store.mutate_share(request).await.unwrap(), receipt);
    assert_guard(&f, 1).await;
    assert_eq!(raw_rows(&f, HISTORY).await, history);
    assert_eq!(
        f.store
            .resource_share_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(f.trace.writes(), 4, "replay must not fence again");
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_artifact_publication_uses_the_same_validated_lineage_fence() {
    let f = fixture().await;
    let history = raw_rows(&f, HISTORY).await;
    let metadata = artifacts::publish(&f, "segmented-artifact").await.unwrap();
    assert_eq!(
        artifacts::publish(&f, "segmented-artifact").await.unwrap(),
        metadata
    );
    assert_guard(&f, 1).await;
    assert_eq!(raw_rows(&f, HISTORY).await, history);
    assert_eq!(
        f.store
            .agent_artifacts
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.store.resource_access_snapshot(&f.target).await.unwrap(),
        f.before
    );
    f.destroy().await;
}

async fn rejects_envelope(fields: Document) {
    let f = fixture().await;
    let agent = f.before.lineage.agent.as_ref().unwrap();
    f.admin
        .database(&f.name)
        .collection::<Document>("agents")
        .update_one(doc! {"_id":agent.id.as_str()}, doc! {"$set":fields})
        .await
        .unwrap();
    let collections = [
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "resource_shares",
        "resource_policies",
        "resource_share_commands",
        "agent_artifacts",
    ];
    let before = raw_rows(&f, &collections).await;
    let history = raw_rows(&f, HISTORY).await;
    f.trace.commands.lock().unwrap().clear();
    assert_eq!(
        f.store.resource_access_snapshot(&f.target).await,
        Err(ResourceAccessStoreError::Corrupt)
    );
    let result = f
        .store
        .mutate_share(mutation(&f.before, "invalid-segmented", ShareMode::View))
        .await;
    assert_eq!(result, Err(ResourceAccessStoreError::Corrupt));
    assert_eq!(
        f.trace.writes(),
        0,
        "invalid envelope must fail before fencing"
    );
    assert_eq!(raw_rows(&f, &collections).await, before);
    assert_eq!(raw_rows(&f, HISTORY).await, history);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unsupported_agent_version_publishes_nothing() {
    rejects_envelope(doc! {"document_schema_version":3_i64}).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn wrong_segmented_incarnation_publishes_nothing() {
    rejects_envelope(doc! {"agent_log_incarnation":"f".repeat(64)}).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_segmented_format_publishes_nothing() {
    rejects_envelope(doc! {"agent_log_format":"unsupported"}).await;
}

async fn stale_revision(collection: &str) {
    let f = fixture().await;
    f.admin
        .database(&f.name)
        .collection::<Document>(collection)
        .update_one(
            doc! {},
            doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
        )
        .await
        .unwrap();
    let names = [
        "sandboxes",
        "workspaces",
        "worktrees",
        "agents",
        "resource_shares",
        "resource_policies",
        "resource_share_commands",
    ];
    let before = raw_rows(&f, &names).await;
    let history = raw_rows(&f, HISTORY).await;
    let mut session = f.store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let result =
        crate::store::resource_access::fence_lineage(&f.store, &f.before, &mut session).await;
    assert!(matches!(
        result,
        Err(crate::store::resource_access::ReadError::Domain(
            ResourceAccessStoreError::Conflict
        ))
    ));
    session.abort_transaction().await.unwrap();
    assert_eq!(
        raw_rows(&f, &names).await,
        before,
        "all preceding fences must abort"
    );
    assert_eq!(raw_rows(&f, HISTORY).await, history);
    f.destroy().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn segmented_agent_revision_cas_still_refuses() {
    stale_revision("agents").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_revision_cas_still_refuses() {
    stale_revision("sandboxes").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn workspace_revision_cas_still_refuses() {
    stale_revision("workspaces").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn worktree_revision_cas_still_refuses() {
    stale_revision("worktrees").await;
}
