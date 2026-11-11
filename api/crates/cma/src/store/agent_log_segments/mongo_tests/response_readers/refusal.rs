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
    let head = Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let filter = doc! {"_id":segments::projection::identity(&head,"output_change","3:0")};
    let original = store
        .segment_logs
        .evidence
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    for mutation in [
        None,
        Some(doc! {"incarnation":"f".repeat(64)}),
        Some(doc! {"head_revision":head.revision+1}),
        Some(doc! {"change_ordinal":9_i64,"record.change.ordinal":9_i64}),
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

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn response_evidence_and_points_keep_the_capture_snapshot_during_changes() {
    let (client, name) = database("response_snapshot").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, rows) = fixtures::source(&store, 2, true).await;
    fixtures::certify(&client, &db, &agent, 2).await;
    let mut captured = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    let head = Head::read(
        store
            .segment_logs
            .heads
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    // Change both evidence and an unread sealed original after capture. This
    // fault fixture proves snapshot consistency, not writer publication.
    assert_eq!(
        store
            .segment_logs
            .evidence
            .delete_one(doc! {"_id":segments::projection::identity(&head,"output_change","3:0")})
            .await
            .unwrap()
            .deleted_count,
        1
    );
    assert_eq!(
        store
            .agent_log
            .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":5_i64})
            .await
            .unwrap()
            .deleted_count,
        1
    );
    let page = captured.changes(0, 1).await.unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].source_sequence, 5);
    assert_eq!(captured.points(&[5]).await.unwrap(), vec![rows[4].clone()]);
    assert_eq!(
        captured.source_page(4, 1).await.unwrap(),
        vec![rows[4].clone()]
    );
    let mut fresh = store
        .agent_output_reader(&agent, Some(3))
        .await
        .unwrap()
        .unwrap();
    assert!(fresh.changes(0, 1).await.is_err());
    drop((captured, fresh));
    db.drop().await.unwrap();
}
