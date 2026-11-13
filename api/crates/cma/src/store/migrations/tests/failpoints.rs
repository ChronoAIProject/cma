use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn ddl_permission_failure_keeps_a_failed_attempt_and_can_resume() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let application = format!("cma-migration-denied-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(application.clone());
    let client = Client::with_options(options).unwrap();
    let database = client.database(&format!("cma_migration_denied_{suffix}"));
    let ledger = ledger::Ledger::new(client, database.clone());
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand",
        "mode":{"times":1_i32},"data":{"failCommands":["createIndexes"],
            "appName":&application,"errorCode":13_i32}})
        .await
        .unwrap();
    let result = apply_indexes(&ledger, &database, &identity()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(result.is_err());
    assert!(!ledger.completed().await.unwrap());
    assert_eq!(activity_indexes::inspect(&database).await.unwrap().len(), 2);
    assert_eq!(
        ledger.status().await.unwrap()["recent_attempts"][0]["outcome"],
        "failed"
    );
    apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    let attempts = ledger.status().await.unwrap()["recent_attempts"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|row| row["outcome"] == "failed"));
    assert!(attempts.iter().any(|row| row["outcome"] == "completed"));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_claim_and_completion_commits_resolve_the_original_attempt() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let application = format!("cma-migration-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(application.clone());
    let client = Client::with_options(options).unwrap();
    let database = client.database(&format!("cma_migration_ambiguity_{suffix}"));
    let ledger = ledger::Ledger::new(client, database.clone());
    ledger.initialize().await.unwrap();
    let injection = doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},
    "data":{"failCommands":["commitTransaction"],"appName":&application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
            "errmsg":"injected migration commit ambiguity"}}};
    admin
        .database("admin")
        .run_command(injection.clone())
        .await
        .unwrap();
    let claimed = ledger.claim(&identity()).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    let lease = claimed.unwrap();
    assert_eq!(
        database
            .collection::<Document>(ledger::COLLECTION)
            .count_documents(doc! {"kind":"attempt"})
            .await
            .unwrap(),
        1
    );
    for index in activity_indexes::indexes() {
        activity_indexes::create(&database, index).await.unwrap();
    }
    admin
        .database("admin")
        .run_command(injection)
        .await
        .unwrap();
    let finished = ledger.finish(&lease, Ok(2)).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    finished.unwrap();
    assert!(ledger.completed().await.unwrap());
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    assert_eq!(
        database
            .collection::<Document>(ledger::COLLECTION)
            .count_documents(doc! {"kind":"attempt"})
            .await
            .unwrap(),
        1
    );
    database.drop().await.unwrap();
}
