use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retired_sandbox_retains_segment_history_sidecars_and_ownership_lineage() {
    let (client, name) = database("segment_retention").await;
    let db = client.database(&name);
    segments::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = retired_sandbox("sbx_segments");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    for (collection, id) in [
        (&store.workspaces, agent.workspace_id.as_str()),
        (&store.worktrees, agent.worktree_id.as_str()),
        (&store.checkpoints, "checkpoint-retained"),
    ] {
        collection
            .insert_one(doc! {"_id":id,"sandbox_id":sandbox.id.as_str()})
            .await
            .unwrap();
    }
    // Healthy, unsupported and damaged envelopes all retain history. A
    // missing sidecar must not turn terminal runtime cleanup into log deletion.
    for version in [2_i64, 3, 1] {
        store
            .agents
            .update_one(
                doc! {"_id":agent.id.as_str()},
                doc! {"$set":{"document_schema_version":version}},
            )
            .await
            .unwrap();
        if version == 1 {
            store.segment_logs.heads.delete_many(doc! {}).await.unwrap();
        }
        let mut before = Vec::new();
        for collection_name in db.list_collection_names().await.unwrap() {
            let rows: Vec<Document> = db
                .collection(&collection_name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            before.push((collection_name, rows));
        }
        assert_eq!(
            store
                .purge_sandbox(&sandbox.id, sandbox.revision, NOW)
                .await,
            Err(SandboxStoreError::Conflict)
        );
        for (collection_name, expected) in before {
            let actual: Vec<Document> = db
                .collection(&collection_name)
                .find(doc! {})
                .sort(doc! {"_id":1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(actual, expected, "retained collection {collection_name}");
        }
    }
    db.drop().await.unwrap();
}
