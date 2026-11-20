use super::*;

async fn fail_channel_command(f: &Fixture, application: &str, command: &str) {
    f.database
        .admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": "alwaysOn",
            "data": {
                "failCommands": [command],
                "appName": application,
                "namespace": format!("{}.eg_channel_records", f.database.name),
                "errorCode": 50_i32,
            },
        })
        .await
        .unwrap();
}

async fn retry_receipt(f: &Fixture, app: &serde_json::Value, body: &str) -> serde_json::Value {
    let response = timeout(
        Duration::from_secs(5),
        f.webhook(
            &webhook_url(f, app),
            "fixture-webhook-secret",
            "restore-transient",
            "issues",
            body,
        )
        .send(),
    )
    .await
    .expect("GitHub receipt request settles within its existing bound")
    .unwrap();
    assert!(response.status().is_success());
    response.json().await.unwrap()
}

pub(super) async fn assert_channel_restore_retry() {
    let (mut f, app, _) = ready_dispatch_fixture().await;
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let application = format!("github-restore-{}", uuid::Uuid::new_v4());
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    let body = issue_event();

    // Refuse the admission fence so matching produces a durable pending delivery.
    fail_channel_command(&f, &application, "update").await;
    let first = retry_receipt(&f, &app, &body).await;
    assert_eq!(first["dispatch"][0]["status"], "pending");
    assert_eq!(first["task_ids"], json!([]));
    assert!(response_posts(&f).is_empty());

    // Error 50 fails the channel read without invalidating the client's topology.
    fail_channel_command(&f, &application, "find").await;
    let mut receipt = first;
    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;
        receipt = retry_receipt(&f, &app, &body).await;
        assert_eq!(receipt["task_ids"], json!([]));
        assert!(response_posts(&f).is_empty());
        if receipt["dispatch"][0]["reason"] == "channel_restore_unavailable" {
            break;
        }
    }
    assert_eq!(
        receipt["dispatch"][0]["reason"], "channel_restore_unavailable",
        "the fault must exercise channel restoration"
    );
    assert_eq!(receipt["dispatch"][0]["status"], "pending");
    let receipt_id = receipt["receipt_id"].clone();
    f.database
        .admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();

    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;
        receipt = retry_receipt(&f, &app, &body).await;
        if receipt["task_ids"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty())
        {
            break;
        }
    }
    assert_eq!(receipt["receipt_id"], receipt_id);
    assert_eq!(receipt["dispatch"][0]["status"], "dispatched");
    assert_eq!(receipt["task_ids"].as_array().unwrap().len(), 1);
    wait_for_posts(&f, 1).await;
    let replay = retry_receipt(&f, &app, &body).await;
    assert_eq!(replay["receipt_id"], receipt_id);
    assert_eq!(replay["task_ids"], receipt["task_ids"]);
    assert_eq!(response_posts(&f).len(), 1);
}
