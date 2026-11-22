use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn baseline_registers_zero_allocation_owners_with_atomic_restartable_cursor() {
    let mut fixture = Fixture::new(false).await;
    let mut sources = Vec::new();
    for owner in ["legacy-free", "legacy-organization"] {
        let mut parent = fixture.parent(owner).await;
        parent.revision += 1;
        parent.runtime = None;
        parent.status = layer2_core::sandbox::SandboxStatus::Failed;
        parent.failure = Some(layer2_core::sandbox::TypedFailure {
            class: "terminal_validation".into(),
            detail: "synthetic setup refusal".into(),
        });
        sources.push(source_id(&parent, 3));
        fixture.store.save_sandbox(1, parent).await.unwrap();
    }
    fixture.prepare().await;
    fixture.store = fixture
        .store
        .with_usage_capture(Some(config()))
        .await
        .unwrap();
    let capture = fixture.capture();
    sources.sort();
    fixture
        .db
        .collection::<Document>(CAPTURES)
        .insert_one(doc! {
            "_id":&sources[0],"document_schema_version":1_i64,
            "stream_id":"conflicting-source-fixture","revision":3_i64,
        })
        .await
        .unwrap();
    assert!(capture.baseline(&fixture.store, MINUTE, 1).await.is_err());
    assert_eq!(fixture.count(INVENTORY).await, 0);
    assert_eq!(fixture.count(CAPTURES).await, 1);
    assert_eq!(fixture.count(STREAMS).await, 0);
    fixture
        .db
        .collection::<Document>(CAPTURES)
        .delete_one(doc! {"_id":&sources[0]})
        .await
        .unwrap();
    capture.baseline(&fixture.store, MINUTE, 1).await.unwrap();
    let first = fixture
        .db
        .collection::<Document>(INVENTORY)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.get_i64("registered").unwrap(), 1);
    assert!(!first.get_bool("complete").unwrap());
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), config())
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        capture.baseline(&fixture.store, MINUTE, 1),
        reopened.baseline(&fixture.store, MINUTE, 1)
    );
    a.unwrap();
    b.unwrap();
    reopened.baseline(&fixture.store, MINUTE, 1).await.unwrap();
    let final_scan = fixture
        .db
        .collection::<Document>(INVENTORY)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(final_scan.get_i64("registered").unwrap(), 2);
    assert_eq!(final_scan.get_i64("owner_kind_unresolved").unwrap(), 2);
    assert!(final_scan.get_bool("complete").unwrap());
    assert_eq!(fixture.count(STREAMS).await, 2);
    assert_eq!(fixture.count(CAPTURES).await, 2);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn explicit_schema_preparation_is_idempotent_and_rejects_unknown_rows_and_indexes() {
    let fixture = Fixture::new(false).await;
    assert_eq!(schema::pending(&fixture.db).await.unwrap(), 11);
    fixture.prepare().await;
    assert!(!schema::apply_next(&fixture.db).await.unwrap());
    fixture
        .db
        .collection::<Document>(CAPTURES)
        .insert_one(doc! {"_id":"future-record","document_schema_version":2_i64})
        .await
        .unwrap();
    assert!(
        CaptureStore::open(&fixture.client, fixture.db.name(), config())
            .await
            .is_err()
    );
    fixture
        .db
        .collection::<Document>(CAPTURES)
        .delete_one(doc! {"_id":"future-record"})
        .await
        .unwrap();
    fixture.db.run_command(doc! {"createIndexes":CAPTURES,"indexes":[{"name":"unreviewed_owner_unique","key":{"owner":1},"unique":true}]}).await.unwrap();
    assert!(
        CaptureStore::open(&fixture.client, fixture.db.name(), config())
            .await
            .is_err()
    );
    assert!(schema::apply_next(&fixture.db).await.is_err());
    fixture.db.drop().await.unwrap();
}
