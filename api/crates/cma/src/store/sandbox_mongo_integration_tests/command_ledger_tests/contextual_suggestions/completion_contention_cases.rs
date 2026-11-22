//! Independent fault variants must all execute after an earlier failure.
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown() {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        1, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_variant_2(
) {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        5, false,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_variant_3(
) {
    suggestion_completion_retries_known_abort_without_regenerating_or_retrying_unknown_case(
        5, true,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn suggestion_claim_known_abort_keeps_original_start() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let name = format!("suggestion_claim_{suffix}");
    let app = format!("suggestion-claim-{suffix}");
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        &suffix,
        "Compare SQLite for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand",
            "mode": {"times": 1_i32}, "data": {"appName":app,
            "failCommands": ["update"], "errorCode":112_i32,
            "errorLabels": ["TransientTransactionError"]},
        })
        .await
        .unwrap();
    let start_ms = unix_millis();
    let outcome = store.claim_suggestions(start_ms).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand", "mode":"off"})
        .await
        .unwrap();
    let reading = read(&router(store.clone()), &agent, "owner").await.1;
    let second = store.claim_suggestions(start_ms).await.unwrap();
    client.database(&name).drop().await.unwrap();
    let claimed = outcome.expect("known abort must retry the claim").unwrap();
    assert_eq!(claimed.started_at_ms, Some(start_ms));
    assert!(claimed
        .deadline_ms
        .is_some_and(|deadline| deadline > start_ms));
    assert_eq!(reading["status"], "running");
    assert!(second.is_none());
}
