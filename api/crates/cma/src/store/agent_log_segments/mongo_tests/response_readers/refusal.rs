use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn indexed_response_refuses_missing_foreign_stale_and_malformed_evidence() {
    let (client, name) = database("response_refuse").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = fixtures::source(&store, 2, false).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    let filter = doc! {"agent_id":agent.id.as_str(),"kind":"output_change"};
    let original = store
        .segment_logs
        .evidence
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    for mutation in [
        None,
        Some(doc! {"incarnation":"foreign"}),
        Some(doc! {"head_revision":i64::MAX}),
        Some(doc! {"record.change.ordinal":9_i64}),
    ] {
        store
            .segment_logs
            .evidence
            .delete_one(filter.clone())
            .await
            .unwrap();
        if let Some(mutation) = mutation {
            store
                .segment_logs
                .evidence
                .insert_one(original.clone())
                .await
                .unwrap();
            store
                .segment_logs
                .evidence
                .update_one(filter.clone(), doc! {"$set":mutation})
                .await
                .unwrap();
        }
        let mut reader = store
            .agent_output_reader(&agent, Some(3))
            .await
            .unwrap()
            .unwrap();
        assert!(reader.changes(0, 8).await.is_err());
        drop(reader);
        store
            .segment_logs
            .evidence
            .replace_one(filter.clone(), original.clone())
            .upsert(true)
            .await
            .unwrap();
    }
    // The same snapshot never turns a failed seek into empty output.
    store
        .segment_logs
        .evidence
        .drop_index("agent_log_output_change_seek")
        .await
        .unwrap();
    let mut reader = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    assert!(reader.changes(0, 8).await.is_err());
    drop(reader);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn indexed_response_refuses_modified_original_points_and_missing_turn() {
    let (client, name) = database("response_points").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = fixtures::source(&store, 2, false).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":3_i64},
            doc! {"$set":{"record.appended_at":"2026-10-01T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert!(store.agent_output_reader(&agent, Some(3)).await.is_err());
    assert!(store.agent_output_reader(&agent, Some(5)).await.is_err());
    db.drop().await.unwrap();
}
