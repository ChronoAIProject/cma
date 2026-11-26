use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn restart_after_pending_receipt_dispatches_once_from_new_gateway_instance() {
    let (mut f, app, _) = ready_dispatch_fixture().await;
    f.children.last_mut().unwrap().kill().await.unwrap();
    let application = format!("github-dispatch-restart-{}", uuid::Uuid::new_v4());
    let previous_uri = f.database.uri.clone();
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    let admin = f.database.admin.database("admin");
    let database = f.database.admin.database(&f.database.name);
    // Stop the first claimant before its admission write can run. The server
    // refuses that write after the pause even after the client disconnects.
    let failpoint = admin
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": {"times": 1},
            "data": {
                "failCommands": ["update"],
                "appName": &application,
                "namespace": format!("{}.eg_channel_records", f.database.name),
                "blockConnection": true,
                "blockTimeMS": 15_000,
                "errorCode": 50,
            },
        })
        .await
        .unwrap();
    let times_entered = failpoint
        .get_i64("count")
        .or_else(|_| failpoint.get_i32("count").map(i64::from))
        .unwrap();
    let (webhook, body) = pending_webhook(&f, &app, "restart-pending").await;
    admin
        .run_command(doc! {
            "waitForFailPoint": "failCommand",
            "timesEntered": times_entered + 1,
            "maxTimeMS": 5_000,
        })
        .await
        .expect("the first claimant reaches the blocked admission write");

    let path = format!(
        "/v1/github/apps/{}/deliveries/restart-pending",
        app["app_ref"].as_str().unwrap()
    );
    let pending = read_receipt(&f, &path).await;
    assert_eq!(pending["dispatch"][0]["status"], "pending");
    assert_eq!(pending["task_ids"], json!([]));
    assert!(response_posts(&f).is_empty());
    let receipt_id = pending["receipt_id"].as_str().unwrap();
    let records = database.collection::<Document>("eg_automation_records");
    let row = records
        .find_one(doc! {"_id": receipt_id})
        .await
        .unwrap()
        .unwrap();
    let lease_until: u64 = row.get_i64("lease_until").unwrap().try_into().unwrap();
    let lease_until = UNIX_EPOCH + Duration::from_secs(lease_until);
    assert!(
        SystemTime::now() < lease_until,
        "original receipt must have a live lease"
    );

    f.children.last_mut().unwrap().kill().await.unwrap();
    // The webhook can already have returned Pending if the background worker
    // acquired the lease. The durable state above proves either claimant stopped.
    let _ = webhook.await.expect("original webhook task completes");
    f.database.uri = previous_uri;
    f.base = f.replica().await;

    let mut receipt = pending.clone();
    let mut observed_live_lease = false;
    let recovered = timeout(Duration::from_secs(45), async {
        loop {
            receipt = read_receipt(&f, &path).await;
            let posts = response_posts(&f).len();
            if SystemTime::now() < lease_until {
                observed_live_lease = true;
                assert_eq!(receipt["receipt_id"], receipt_id);
                assert_eq!(receipt["dispatch"][0]["status"], "pending");
                assert_eq!(receipt["task_ids"], json!([]));
                assert_eq!(
                    posts, 0,
                    "replacement ran before the original lease expired"
                );
            } else if receipt["dispatch"][0]["status"] == "dispatched" {
                break;
            }
            sleep(Duration::from_millis(150)).await;
        }
    })
    .await;
    assert!(
        observed_live_lease,
        "replacement must observe the original live lease"
    );
    assert!(
        recovered.is_ok(),
        "replacement did not settle the receipt after lease expiry (status={}, reason={})",
        receipt["dispatch"][0]["status"],
        receipt["dispatch"][0]["reason"],
    );
    assert_eq!(receipt["receipt_id"], receipt_id);
    assert_eq!(receipt["task_ids"].as_array().unwrap().len(), 1);
    wait_for_posts(&f, 1).await;

    let replay = f
        .webhook(
            &webhook_url(&f, &app),
            "fixture-webhook-secret",
            "restart-pending",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    let replay: serde_json::Value = replay.json().await.unwrap();
    assert_eq!(replay["receipt_id"], receipt_id);
    assert_eq!(replay["task_ids"], receipt["task_ids"]);
    assert_eq!(response_posts(&f).len(), 1);
}

async fn read_receipt(f: &Fixture, path: &str) -> serde_json::Value {
    let response = f.read(&f.base, path, "token-alice").await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}
