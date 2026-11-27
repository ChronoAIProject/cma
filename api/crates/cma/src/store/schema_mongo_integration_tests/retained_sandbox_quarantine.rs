use layer2_core::sandbox::store::{
    PageRequest, SandboxStore, SandboxStoreError, SandboxWorkerStore,
};
use layer2_core::sandbox::{SandboxAdminQuery, SandboxAdminStore};
use mongodb::bson::{doc, Document};

use super::database;
use crate::store::{
    runtime_routing::MongoRuntimeRouting,
    sandbox_mongo::{MongoSandboxStore, SANDBOXES},
    sandbox_mongo_integration_tests::sandbox,
    sandbox_quarantine::FIELD,
    sandboxes::{read_sandbox, sandbox_document},
    work_schema,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn startup_quarantines_the_v3_machine_origin_and_serves_healthy_sandboxes() {
    let (client, name) = database().await;
    let db = client.database(&name);
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let broken = sandbox("sbx_a_broken");
    let mut healthy = sandbox("sbx_b_healthy");
    healthy.name = "healthy".into();
    store.create_sandbox(healthy.clone()).await.unwrap();
    let rows = db.collection::<Document>(SANDBOXES);
    let mut raw = sandbox_document(&broken, 0).unwrap();
    raw.insert("status", "paused");
    raw.insert("document_schema_version", work_schema::RENEWED_VERSION);
    let aggregate = raw.get_document_mut("record").unwrap();
    aggregate.insert("status", "paused");
    aggregate
        .get_document_mut("lifecycle")
        .unwrap()
        .insert("runtime_image_origin", "managed_machine");
    aggregate
        .get_document_mut("work")
        .unwrap()
        .insert("authority_protocol", "renewed_v1");
    assert!(
        matches!(read_sandbox(&raw), Err(SandboxStoreError::Corrupt(reason))
        if reason == "runtime image origin disagrees with ordinary-work envelope")
    );
    rows.insert_one(raw.clone()).await.unwrap();

    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let routes = db.collection::<Document>("runtime_backend_routes");
    let query = doc! {"_id":broken.id.as_str()};
    let mut marker = None;
    for _ in 0..2 {
        MongoRuntimeRouting::open(&db, "fixture", false)
            .await
            .unwrap();
        let mut stored = rows.find_one(query.clone()).await.unwrap().unwrap();
        let detected = stored.remove(FIELD).expect("durable quarantine marker");
        if let Some(previous) = marker.replace(detected.clone()) {
            assert_eq!(previous, detected);
        }
        assert_eq!(stored, raw);
        assert_eq!(
            routes
                .count_documents(doc! {"sandbox_id": broken.id.as_str()})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            routes
                .count_documents(doc! {"sandbox_id": healthy.id.as_str()})
                .await
                .unwrap(),
            1
        );
    }
    let page = PageRequest::new(1, None).unwrap();
    assert_eq!(
        store
            .list_sandboxes(&healthy.owner, &page)
            .await
            .unwrap()
            .items,
        [healthy.clone()]
    );
    let admin = store
        .list_admin_sandboxes(&SandboxAdminQuery {
            page,
            owner: Some(healthy.owner.clone()),
            status: None,
            runtime_version: None,
            always_on: None,
        })
        .await
        .unwrap();
    assert_eq!(admin.items.len(), 1);
    assert_eq!(admin.items[0].id, healthy.id);
    assert!(matches!(store.get_sandbox(&broken.id).await,
        Err(SandboxStoreError::Corrupt(reason)) if reason == "retained Sandbox is quarantined"));
    assert_eq!(
        store
            .due_sandbox_work(&healthy.work.ready_at, 1)
            .await
            .unwrap(),
        [healthy.id]
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn lease_replacement_retains_the_machine_origin_envelope() {
    let (client, name) = database().await;
    let db = client.database(&name);
    let rows = db.collection::<Document>(SANDBOXES);
    rows.insert_one(doc! {"_id":"previous","document_schema_version":work_schema::RENEWED_VERSION})
        .await
        .unwrap();
    let updated = doc! {
        "document_schema_version": work_schema::MANAGED_MACHINE_ORIGIN_VERSION,
        "record": {"lifecycle": {"runtime_image_origin":"managed_machine"}, "work":{}},
    };
    rows.update_one(
        doc! {"_id":"previous"},
        crate::store::work_lease_update::replacement(updated),
    )
    .await
    .unwrap();
    let row = rows
        .find_one(doc! {"_id":"previous"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_i64("document_schema_version").unwrap(),
        work_schema::MANAGED_MACHINE_ORIGIN_VERSION
    );
    assert!(work_schema::read::<Document>(&row).is_ok());
    db.drop().await.unwrap();
}
