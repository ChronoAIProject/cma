use super::*;

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn additive_units_resume_and_reject_index_order_drift_before_mutation() {
    let (client, name) = database("ddl_steps").await;
    let db = client.database(&name);
    assert!(schema::pending_count(&db).await.unwrap() > 0);
    assert!(schema::apply_next(&db).await.unwrap());
    assert!(schema::verify(&db).await.is_err());
    schema::migrate(&db).await.unwrap();
    assert!(!schema::apply_next(&db).await.unwrap());
    assert_eq!(schema::pending_count(&db).await.unwrap(), 0);
    db.collection::<Document>(SEGMENTS)
        .drop_index("agent_log_segment_start")
        .await
        .unwrap();
    db.collection::<Document>(SEGMENTS)
        .create_index(
            mongodb::IndexModel::builder()
                .keys(doc! {"start_seq":1,"incarnation":1,"agent_id":1})
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name("agent_log_segment_start".to_owned())
                        .unique(true)
                        .build(),
                )
                .build(),
        )
        .await
        .unwrap();
    db.collection::<Document>(HEADS).drop().await.unwrap();
    assert!(schema::apply_next(&db).await.is_err());
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|n| n == HEADS));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn reader_does_not_create_sidecars_and_requires_them_for_activated_envelopes() {
    let (client, name) = database("startup").await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|n| COLLECTIONS.contains(&n.as_str())));
    let mut row = crate::store::agents::agent_document(&agent()).unwrap();
    row.insert("document_schema_version", 2_i64);
    row.insert("agent_log_format", FORMAT);
    row.insert("agent_log_incarnation", digest::incarnation(&agent()));
    row.insert("agent_log_head_revision", 1_i64);
    store.agents.insert_one(row).await.unwrap();
    assert!(schema::verify(&db).await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn publication_schema_accepts_pinned_receipts_and_requires_original_time() {
    use segments::publication::{Publication, PublicationIdentity};
    let (client, name) = database("receipt_schema").await;
    schema::migrate(&client.database(&name)).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (current, entries) = segmented(&store, 1).await;
    let mut head = Head::read(
        collection(&store, HEADS)
            .find_one(doc! {"_id":current.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut next = current.clone();
    next.revision += 1;
    next.conversation.last_seq += 1;
    let event = AgentLogEntry {
        seq: next.conversation.last_seq,
        ..entries.last().unwrap().clone()
    };
    let bytes = bson::serialize_to_vec(&log_document(&event).unwrap())
        .unwrap()
        .len() as i64;
    head.revision += 1;
    head.last_seq += 1;
    head.active_count += 1;
    head.active_bytes += bytes;
    head.lifetime_count += 1;
    head.lifetime_bytes += bytes;
    head.projection_through += 1;
    let operation = PublicationIdentity {
        key: "schema-fixture-operation".into(),
        fingerprint: digest::binary(vec![5; 32]),
        appended_at: NOW.into(),
    };
    let receipt = Publication::new(
        &current,
        &next,
        &head,
        &operation,
        digest::binary(vec![7; 32]),
    )
    .unwrap();
    let row = bson::serialize_to_document(&receipt).unwrap();
    let receipts = collection(&store, PUBLICATIONS);
    receipts.insert_one(row.clone()).await.unwrap();
    let stored = receipts
        .find_one(doc! {"_id":&receipt.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Publication::read(stored).unwrap().appended_at, NOW);
    for case in 0..3 {
        let mut invalid = row.clone();
        invalid.insert("_id", format!("invalid-receipt-{case}"));
        invalid.insert("publication_key", format!("invalid-operation-{case}"));
        match case {
            0 => {
                invalid.remove("appended_at");
            }
            1 => {
                invalid.insert("appended_at", 1_i64);
            }
            _ => {
                invalid.insert("unrecognized", true);
            }
        }
        let error = receipts.insert_one(invalid).await.unwrap_err();
        assert!(
            matches!(*error.kind,mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(ref e)) if e.code==121)
        );
    }
    assert_eq!(receipts.count_documents(doc! {}).await.unwrap(), 1);
    client.database(&name).drop().await.unwrap();
}
