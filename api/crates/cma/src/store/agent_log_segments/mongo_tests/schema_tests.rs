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
                        .name("agent_log_segment_start".into())
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
