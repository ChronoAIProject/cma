use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_lifecycle_startup_creates_collection_and_refuses_unknown_versions() {
    const LIFECYCLE: &str = "cma_document_lifecycle";
    let (client, name) = database().await;
    let key = auth_key();
    let open = || {
        MongoStores::open(
            client.clone(),
            &name,
            "document-lifecycle-schema",
            &key,
            Duration::from_secs(86_400),
            std::sync::Arc::new(super::super::checkpoint_storage::RetiredCheckpointStorage),
            ("runtime-lifecycle-schema", None),
        )
    };
    open().await.unwrap();
    assert!(client
        .database(&name)
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|name| name == LIFECYCLE));
    insert(
        &client,
        &name,
        LIFECYCLE,
        doc! {"_id": "unsupported-lifecycle", "document_schema_version": 2_i64},
    )
    .await;
    assert!(matches!(
        open().await,
        Err(StoreError::Malformed(detail)) if detail.contains(LIFECYCLE)
    ));
    client.database(&name).drop().await.unwrap();
}
