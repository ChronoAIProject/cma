use super::*;
use layer2_core::admission::AdmissionControlStore;

async fn stop(client: &mongodb::Client, database: &str) {
    crate::store::admission_control::MongoAdmissionControlStore::open(client, database)
        .await
        .unwrap()
        .change(
            0,
            &layer2_core::SubjectId::new("operator"),
            "contain incident",
            true,
            100,
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn fresh_binding_reprovision_is_paused_without_archiving_or_replacing_resources() {
    let (client, database, store, _, controller, data) = fixture().await;
    let before = store
        .bindings
        .records
        .find_one(doc! {"_id": &data.binding.binding_id})
        .await
        .unwrap();
    stop(&client, &database).await;
    assert!(matches!(
        controller
            .prepare_retired_binding(&data.intent, &data.key)
            .await,
        Err(CoreError::AdmissionPaused)
    ));
    assert_eq!(
        store
            .bindings
            .records
            .find_one(doc! {"_id": &data.binding.binding_id})
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        store.get_sandbox(&data.sandbox.id).await.unwrap(),
        Some(data.sandbox)
    );
    assert_eq!(
        store.get_workspace(&data.workspace.id).await.unwrap(),
        Some(data.workspace)
    );
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_binding_reprovision_replays_its_exact_receipt_and_continues_after_stop() {
    let (client, database, store, custody, controller, data) = fixture().await;
    let receipt = controller
        .prepare_retired_binding(&data.intent, &data.key)
        .await
        .unwrap();
    stop(&client, &database).await;
    let reopened = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let replacement = MongoBindingCredentials::new(reopened.clone(), custody);
    assert_eq!(
        replacement
            .prepare_retired_binding(&data.intent, &data.key)
            .await
            .unwrap(),
        receipt
    );
    let mut progressed = receipt.reserved.clone();
    progressed.work = BindingWork::ProvisionWorkspace;
    progressed
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    reopened
        .save_binding(Some(receipt.reserved.revision), progressed.clone())
        .await
        .unwrap();
    assert_eq!(
        controller
            .prepare_retired_binding(&data.intent, &data.key)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        store.binding(&progressed.binding_id).await.unwrap(),
        Some(progressed)
    );
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}
