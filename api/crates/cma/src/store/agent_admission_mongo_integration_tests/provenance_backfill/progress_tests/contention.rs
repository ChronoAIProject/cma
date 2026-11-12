use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn progress_exhausted_transaction_retries_preserve_effect_cursor_and_counts() {
    let (admin, name, _, _, _, request) = legacy_fixture("progress_exhausted").await;
    let db = admin.database(&name);
    initialize(&db).await.unwrap();
    let app = worker();
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = counts(&store).await;
    let before = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand",
            "mode":{"times":crate::store::sandbox_mongo::TRANSACTION_ATTEMPTS as i32},
            "data":{"failCommands":["findAndModify"],"appName":&app,
                "errorCode":112_i32,"errorLabels":["TransientTransactionError"]}
        })
        .await
        .unwrap();
    let outcome = store.durable_provenance_backfill_step(&app).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
    assert_eq!(counts(&store).await, original);
    let after = store
        .agent_provenance_snapshot(&request.agent_id)
        .await
        .unwrap();
    assert_eq!(after.resource, before.resource);
    assert_eq!(after.first_log, before.first_log);
    assert!(after.creation.is_none());
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store.durable_provenance_backfill_step(&app).await.unwrap(),
        Some(ProvenanceBackfillProgress::Advanced)
    );
    let committed = counts(&store).await;
    assert_eq!(
        committed.get_str("cursor").unwrap(),
        request.agent_id.as_str()
    );
    assert_eq!(
        committed.get_document("counts").unwrap(),
        &doc! {"scanned":1_i64,"recorded":1_i64,"retained":0_i64,"skipped":0_i64,"errors":0_i64}
    );
    assert_eq!(
        store
            .agent_creation_provenance
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}
