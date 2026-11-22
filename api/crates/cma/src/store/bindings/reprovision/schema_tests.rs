use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_schema_guard_preparation_refuses_new_work_then_resumes_original_intent() {
    let (client, name, store, custody, _, data) = fixture().await;
    let db = client.database(&name);
    db.collection::<Document>(PREPARATIONS)
        .drop_index("binding_id_1_generation_1")
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let controller = MongoBindingCredentials::new(reopened, custody.clone());
    let result = controller
        .prepare_retired_binding(&data.intent, &data.key)
        .await;
    let saved = store.binding(&data.binding.binding_id).await.unwrap();
    let count = store
        .bindings
        .preparations
        .count_documents(doc! {})
        .await
        .unwrap();
    let index = crate::store::bindings::schema::indexes(PREPARATIONS).remove(0);
    db.run_command(doc! {"createIndexes":PREPARATIONS,"indexes":[index]})
        .await
        .unwrap();
    let recovered = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let controller = MongoBindingCredentials::new(recovered, custody.clone());
    let receipt = controller
        .prepare_retired_binding(&data.intent, &data.key)
        .await
        .unwrap();
    assert_eq!(receipt.intent, data.intent);
    // An already acknowledged immutable receipt remains replayable by exact ID.
    db.collection::<Document>(PREPARATIONS)
        .drop_index("binding_id_1_generation_1")
        .await
        .unwrap();
    let reopened = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let controller = MongoBindingCredentials::new(reopened, custody);
    assert_eq!(
        controller
            .prepare_retired_binding(&data.intent, &data.key)
            .await
            .unwrap(),
        receipt
    );
    db.drop().await.unwrap();
    assert!(
        matches!(result, Err(CoreError::RuntimeUnavailable(_))),
        "{result:?}"
    );
    assert_eq!(count, 0);
    assert_eq!(saved, Some(data.binding));
}
