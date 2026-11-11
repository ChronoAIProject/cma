use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn quarantine_and_incomplete_envelopes_refuse_without_repairing_source() {
    let (client, name) = database("quarantine").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    let filter = doc! {"_id":agent.id.as_str()};
    let original = store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let baseline_rows = store.agent_log.count_documents(doc! {}).await.unwrap();
    let updates = [
        doc! {"$set":{"agent_log_integrity_quarantine_v1":{"format":1_i64}}},
        doc! {"$unset":{"agent_log_incarnation":""}},
        doc! {"$set":{"agent_log_incarnation":"z".repeat(64)}},
        doc! {"$set":{"agent_log_head_revision":2_i64}},
        doc! {"$set":{"document_schema_version":1_i64}},
        doc! {"$set":{"document_schema_version":3_i64}},
    ];
    for update in updates {
        store
            .agents
            .update_one(filter.clone(), update)
            .await
            .unwrap();
        let before = store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(store.agent_log_range(&agent.id, 0, 2, 2).await.is_err());
        assert!(crate::store::agent_log_store::point(&store, &agent.id, 2)
            .await
            .is_err());
        assert_eq!(
            store
                .agents
                .find_one(filter.clone())
                .await
                .unwrap()
                .unwrap(),
            before
        );
        assert_eq!(
            store.agent_log.count_documents(doc! {}).await.unwrap(),
            baseline_rows
        );
        store
            .agents
            .replace_one(filter.clone(), original.clone())
            .await
            .unwrap();
    }
    assert_eq!(
        store
            .agent_log_range(&agent.id, 0, 2, 2)
            .await
            .unwrap()
            .len(),
        2
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn point_reads_verify_original_sealed_bytes_and_empty_pages_verify_active_tail() {
    let (client, name) = database("point_proof").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    assert_eq!(
        crate::store::agent_log_store::point(&store, &agent.id, 2)
            .await
            .unwrap(),
        Some(entries[1].clone())
    );
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":2_i64},
            doc! {"$set":{"record.appended_at":"2026-11-02T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert!(crate::store::agent_log_store::point(&store, &agent.id, 2)
        .await
        .is_err());
    assert!(crate::store::agent_log_store::point(&store, &agent.id, 9)
        .await
        .unwrap()
        .is_none());
    store
        .agent_log
        .delete_one(doc! {"agent_id":agent.id.as_str(),"seq":8_i64})
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 8, 8, 1).await.is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn predecessor_and_range_share_one_snapshot_and_check_undelivered_rows() {
    let (client, name) = database("visitor").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 3).await;
    let mut seen = Vec::new();
    crate::store::agent_log_store::visit_page(&store, &agent.id, 2, 8, 3, true, &mut |entry| {
        seen.push(entry.seq);
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(seen, vec![2, 3, 4, 5]);
    let mut delivered = None;
    store
        .agent_log
        .update_one(
            doc! {"agent_id":agent.id.as_str(),"seq":4_i64},
            doc! {"$set":{"record.appended_at":"2026-11-02T00:00:00Z"}},
        )
        .await
        .unwrap();
    assert!(crate::store::agent_log_store::visit_page(
        &store,
        &agent.id,
        0,
        6,
        6,
        false,
        &mut |entry| {
            if delivered.is_none() {
                delivered = Some(entry);
            }
            Ok(())
        }
    )
    .await
    .is_err());
    assert_eq!(delivered, Some(entries[0].clone()));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn segmented_history_never_enters_the_legacy_preparation_scanner() {
    let (client, name) = database("legacy_guard").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = segmented(&store, 3).await;
    let before = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let result = crate::store::agent_log_integrity::scan_complete_log_with_snapshot_session(
        crate::store::agent_log_telemetry::ScanCaller::PublicWindowPrefix,
        &store,
        &agent.id,
        agent.conversation.last_seq,
        &mut session,
        |_| panic!("legacy scan must not start"),
    )
    .await;
    assert!(matches!(
        result,
        Err(
            crate::store::agent_log_integrity::TransactionLogError::Domain(
                layer2_core::sandbox::store::SandboxStoreError::Unavailable(_)
            )
        )
    ));
    assert_eq!(
        store
            .agents
            .find_one(doc! {"_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        before
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn unsealed_creation_lineage_is_checked_when_the_page_does_not_deliver_it() {
    let (client, name) = database("creation_lineage").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, entries) = segmented(&store, 0).await;
    assert_eq!(
        store.agent_log_range(&agent.id, 1, 2, 1).await.unwrap(),
        entries[1..]
    );
    let original = log_document(&entries[0]).unwrap();
    let mut changed = entries[0].clone();
    if let AgentLogKind::Created { workspace_id, .. } = &mut changed.kind {
        workspace_id.0 = workspace_id.0.to_uppercase();
    }
    let corrupted = log_document(&changed).unwrap();
    assert_eq!(
        bson::serialize_to_vec(&original).unwrap().len(),
        bson::serialize_to_vec(&corrupted).unwrap().len()
    );
    store
        .agent_log
        .replace_one(
            doc! {"agent_id":agent.id.as_str(),"seq":1_i64},
            corrupted.clone(),
        )
        .await
        .unwrap();
    assert!(store.agent_log_range(&agent.id, 1, 2, 1).await.is_err());
    assert!(store.agent_log_range(&agent.id, 2, 2, 1).await.is_err());
    assert_eq!(
        store
            .agent_log
            .find_one(doc! {"agent_id":agent.id.as_str(),"seq":1_i64})
            .await
            .unwrap(),
        Some(corrupted)
    );
    client.database(&name).drop().await.unwrap();
}
