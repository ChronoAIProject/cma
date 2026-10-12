use super::*;
use crate::store::{legacy_history, sandbox_mongo::start_transaction};
use layer2_core::profiles::content_digest;
use mongodb::bson::{Bson, Document};
use serde_json::json;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn legacy_late_upload_reference_is_atomic_replayable_and_keeps_its_digest() {
    let (client, _, name) = test_store("legacy_history").await;
    let db = client.database(&name);
    let record = json!({
        "id":"allocation", "identity":{"partition":"partition"},
        "owner":{"type":"person","subject":"owner"},"pin":{"revision":2},
        "config":{"keep":true}, "first_source":{"sequence":3},
        "placement":{"target":{"sandbox_id":"sbx_legacy"}},
        "operation_id":"operation", "created_at_ms":1_700_000_000_000_i64
    });
    let material = json!([
        "channel-history-allocation-v1",
        record["identity"],
        record["owner"],
        record["pin"],
        record["config"],
        record["first_source"],
        record["placement"],
        record["operation_id"],
        null,
        record["created_at_ms"]
    ]);
    let rows = db.collection::<Document>("bridge_allocations");
    rows.insert_one(doc! {"_id":"allocation","document_schema_version":crate::store::schema::DOCUMENT_SCHEMA_VERSION,
        "creates_sandbox":true,"record":mongodb::bson::serialize_to_bson(&record).unwrap(),
        "channel_history_reference_count":0_i64,
        "channel_history_policy":{"allocation_id":"allocation","allocation_sha256":content_digest(&material).unwrap(),"disposition":"retain"}
    }).await.unwrap();
    let tombstone = doc! {"_id":"late", "document_schema_version":1_i32,"object_key":"legacy/home.cma","attempts":3_i32};
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    legacy_history::tombstone(&db, "sbx_legacy", &tombstone, &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    let references = db.collection::<Document>("channel_history_references");
    assert_eq!(references.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(
        rows.find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("channel_history_reference_count")
            .unwrap(),
        0
    );
    for _ in 0..2 {
        start_transaction(&mut session).await.unwrap();
        legacy_history::tombstone(&db, "sbx_legacy", &tombstone, &mut session)
            .await
            .unwrap();
        session.commit_transaction().await.unwrap();
    }
    assert_eq!(references.count_documents(doc! {}).await.unwrap(), 1);
    let retained = references.find_one(doc! {}).await.unwrap().unwrap();
    let projection =
        doc! {"_id":"late", "document_schema_version":1_i32,"object_key":"legacy/home.cma"};
    assert_eq!(
        retained
            .get_document("record")
            .unwrap()
            .get_str("target_sha256")
            .unwrap(),
        content_digest(&Bson::Document(projection)).unwrap()
    );
    assert_eq!(
        rows.find_one(doc! {})
            .await
            .unwrap()
            .unwrap()
            .get_i64("channel_history_reference_count")
            .unwrap(),
        1
    );
    let mut changed = tombstone.clone();
    changed.insert("object_key", "different/home.cma");
    start_transaction(&mut session).await.unwrap();
    assert!(
        legacy_history::tombstone(&db, "sbx_legacy", &changed, &mut session)
            .await
            .is_err()
    );
    session.abort_transaction().await.unwrap();
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB"]
async fn restore_refuses_retained_legacy_removal_evidence() {
    let (client, _, name) = test_store("legacy_restore").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let sandbox = sandbox("sbx_legacy_restore");
    store.create_sandbox(sandbox.clone()).await.unwrap();
    assert!(store.retired_runtime_cleanup_plan(&sandbox).await.is_ok());
    let db = client.database(&name);
    for collection in [
        "channel_reuse_cleanup_tombstones",
        "owned_workspace_cleanup_tombstones_v1",
    ] {
        let rows = db.collection::<Document>(collection);
        rows.insert_one(
            doc! {"_id":"retained","record":{"admission":{"sandbox_id":sandbox.id.as_str()}}},
        )
        .await
        .unwrap();
        assert!(store.retired_runtime_cleanup_plan(&sandbox).await.is_err());
        rows.delete_one(doc! {"_id":"retained"}).await.unwrap();
    }
    db.drop().await.unwrap();
}
