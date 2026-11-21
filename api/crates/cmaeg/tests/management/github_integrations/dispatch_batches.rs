use super::{sources, *};
use cmaeg_core::seal::{open_blob, seal_blob, MasterKeyRing};
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Document};
use std::collections::BTreeMap;
use tokio::time::{sleep, timeout, Duration};

const DELIVERY: &str = "batch-recovery";

async fn stop(f: &mut Fixture) {
    for mut child in f.children.drain(..) {
        child.kill().await.unwrap();
    }
}

async fn fanout(count: usize) -> (Fixture, serde_json::Value) {
    let (mut f, app, _) = sources::ready_dispatch_fixture().await;
    for index in 1..count {
        let mut body = sources::channel(&app);
        body["name"] = format!("batch-channel-{index}").into();
        let response = f
            .client
            .post(format!("{}/v1/channels", f.base))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let channel: serde_json::Value = response.json().await.unwrap();
        let channel = sources::enable_routing(
            &f,
            channel["channel_id"].as_str().unwrap(),
            &channel["version"],
        )
        .await;
        f.routed_records
            .push(channel["record_id"].as_str().unwrap().to_owned());
    }
    stop(&mut f).await;
    f.base = f.replica().await;
    (f, app)
}

async fn webhook(f: &Fixture, app: &serde_json::Value) -> serde_json::Value {
    let response = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            DELIVERY,
            "issues",
            &sources::issue_event(),
        )
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    response.json().await.unwrap()
}

async fn wait_receipt(
    f: &Fixture,
    app: &serde_json::Value,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    timeout(Duration::from_secs(40), async {
        loop {
            let response = f
                .read(
                    &f.base,
                    &format!(
                        "/v1/github/apps/{}/deliveries/{DELIVERY}",
                        app["app_ref"].as_str().unwrap()
                    ),
                    "token-alice",
                )
                .await;
            assert_eq!(response.status(), 200);
            let receipt = response.json().await.unwrap();
            if ready(&receipt) {
                return receipt;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("every eligible channel makes durable progress")
}

fn all_dispatched(receipt: &serde_json::Value) -> bool {
    receipt["dispatch"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["status"] == "dispatched")
}

fn identities(receipt: &serde_json::Value) -> BTreeMap<String, String> {
    receipt["dispatch"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| {
            entry["admission_id"].as_str().map(|id| {
                (
                    entry["channel_id"].as_str().unwrap().to_owned(),
                    id.to_owned(),
                )
            })
        })
        .collect()
}

async fn assert_posts(f: &Fixture, expected: usize) {
    timeout(Duration::from_secs(40), async {
        loop {
            let posts = f
                .provider
                .requests()
                .into_iter()
                .filter(|request| request.path == "/api/v2/responses")
                .count();
            assert!(
                posts <= expected,
                "a retained admission must not execute twice"
            );
            if posts == expected {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
}

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD_NO_PAD.encode([1_u8; 32]))).unwrap()
}

async fn read_sealed(f: &Fixture, collection: &str, id: &str) -> serde_json::Value {
    let row = f
        .database
        .admin
        .database(&f.database.name)
        .collection::<Document>(collection)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    let opened = open_blob(&keys(), row.get_binary_generic("blob").unwrap()).unwrap();
    serde_json::from_slice(&opened.payload).unwrap()
}

async fn write_sealed(
    f: &Fixture,
    collection: &str,
    id: &str,
    value: &serde_json::Value,
    extra: Document,
) {
    let bytes = seal_blob(&keys(), &serde_json::to_vec(value).unwrap()).unwrap();
    let mut fields = doc! {"blob":Binary {subtype:BinarySubtype::Generic,bytes}};
    fields.extend(extra);
    let result = f
        .database
        .admin
        .database(&f.database.name)
        .collection::<Document>(collection)
        .update_one(doc! {"_id":id}, doc! {"$set":fields})
        .await
        .unwrap();
    assert_eq!(result.matched_count, 1);
}

async fn restore_pending(
    f: &Fixture,
    receipt: &serde_json::Value,
    future_prefix: bool,
) -> serde_json::Value {
    let id = receipt["receipt_id"].as_str().unwrap();
    let mut sealed = read_sealed(f, "eg_automation_records", id).await;
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let value = &mut sealed["value"];
    value["receipt"]["status"] = "pending".into();
    value["receipt"]["task_ids"] = json!([]);
    for (index, entry) in value["dispatch"]["channels"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        entry["status"] = "pending".into();
        entry["admission_id"] = serde_json::Value::Null;
        entry["reason"] = serde_json::Value::Null;
        entry["attempts"] = 0.into();
        entry["next_attempt_at"] = (now
            + if future_prefix && index < 16 {
                3600
            } else {
                -1
            })
        .into();
    }
    value["receipt"]["dispatch"] = value["dispatch"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| json!({"channel_id":entry["channel_id"],"status":"pending"}))
        .collect();
    // Simulate a terminated claimant and an expired lease; retained admission roots stay intact.
    write_sealed(
        f,
        "eg_automation_records",
        id,
        &sealed,
        doc! {"next_dispatch_at":now-1,"lease_until":now-1},
    )
    .await;
    sealed
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn receipt_fanout_settles_every_channel_across_batches_and_restart() {
    let (mut f, app) = fanout(33).await;
    let first = webhook(&f, &app).await;
    assert_eq!(first["dispatch"].as_array().unwrap().len(), 33);
    let receipt = wait_receipt(&f, &app, all_dispatched).await;
    assert_eq!(receipt["status"], "accepted");
    assert_eq!(receipt["task_ids"].as_array().unwrap().len(), 33);
    let tasks = identities(&receipt);
    assert_eq!(tasks.len(), 33);
    assert_eq!(
        tasks
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        33
    );
    assert_posts(&f, 33).await;
    stop(&mut f).await;
    f.base = f.replica().await;
    let replay = webhook(&f, &app).await;
    assert_eq!(replay["receipt_id"], receipt["receipt_id"]);
    assert_eq!(identities(&replay), tasks);
    assert!(all_dispatched(&replay));
    assert_posts(&f, 33).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn future_prefix_yields_after_lease_takeover_and_revoked_channel_is_refused() {
    let (mut f, app) = fanout(17).await;
    webhook(&f, &app).await;
    let original = wait_receipt(&f, &app, all_dispatched).await;
    let tasks = identities(&original);
    assert_posts(&f, 17).await;
    stop(&mut f).await;
    restore_pending(&f, &original, true).await;
    f.base = f.replica().await;
    f.replica().await;
    let partial = wait_receipt(&f, &app, |receipt| {
        receipt["dispatch"][16]["status"] == "dispatched"
    })
    .await;
    assert_eq!(partial["status"], "accepted");
    assert_eq!(partial["task_ids"].as_array().unwrap().len(), 1);
    assert!(partial["dispatch"].as_array().unwrap()[..16]
        .iter()
        .all(|entry| entry["status"] == "pending"));
    for (channel, task) in identities(&partial) {
        assert_eq!(tasks[&channel], task);
    }

    let revoked = original["dispatch"][0]["channel_id"].as_str().unwrap();
    let current: serde_json::Value = f
        .read(&f.base, &format!("/v1/channels/{revoked}"), "token-alice")
        .await
        .json()
        .await
        .unwrap();
    let mut update = sources::channel(&app);
    update.as_object_mut().unwrap().remove("credentials");
    update["expected_version"] = current["version"].clone();
    update["sources"] = super::super::create_body()["sources"].clone();
    let response = f
        .client
        .put(format!("{}/v1/channels/{revoked}", f.base))
        .bearer_auth("token-alice")
        .json(&update)
        .send()
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(status, 200, "{}", response.text().await.unwrap());

    stop(&mut f).await;
    let id = original["receipt_id"].as_str().unwrap();
    let mut sealed = read_sealed(&f, "eg_automation_records", id).await;
    for entry in sealed["value"]["dispatch"]["channels"]
        .as_array_mut()
        .unwrap()
    {
        entry["next_attempt_at"] = 0.into();
    }
    write_sealed(
        &f,
        "eg_automation_records",
        id,
        &sealed,
        doc! {"next_dispatch_at":0_i64,"lease_until":0_i64},
    )
    .await;
    f.base = f.replica().await;
    let settled = wait_receipt(&f, &app, |receipt| {
        receipt["dispatch"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["status"] != "pending")
    })
    .await;
    assert_eq!(settled["dispatch"][0]["status"], "refused");
    assert_eq!(
        settled["dispatch"][0]["reason"],
        "channel_changed_before_admission"
    );
    assert_eq!(settled["task_ids"].as_array().unwrap().len(), 16);
    let mut expected = tasks;
    expected.remove(revoked);
    assert_eq!(identities(&settled), expected);
    assert_posts(&f, 17).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn retryable_restore_prefix_leaves_progress_for_later_channels() {
    let (mut f, app) = fanout(17).await;
    webhook(&f, &app).await;
    let original = wait_receipt(&f, &app, all_dispatched).await;
    let tasks = identities(&original);
    assert_posts(&f, 17).await;
    stop(&mut f).await;
    let sealed = restore_pending(&f, &original, false).await;
    let mut records = Vec::new();
    for entry in &sealed["value"]["dispatch"]["channels"].as_array().unwrap()[..16] {
        let id = entry["record_id"].as_str().unwrap();
        let saved = read_sealed(&f, "eg_channel_records", id).await;
        let mut unavailable = saved.clone();
        unavailable["creation"]["ready"] = false.into();
        write_sealed(&f, "eg_channel_records", id, &unavailable, doc! {}).await;
        records.push((id.to_owned(), saved));
    }
    f.base = f.replica().await;
    let partial = wait_receipt(&f, &app, |receipt| {
        receipt["dispatch"][16]["status"] == "dispatched"
    })
    .await;
    assert_eq!(partial["status"], "accepted");
    assert_eq!(partial["task_ids"].as_array().unwrap().len(), 1);
    for entry in &partial["dispatch"].as_array().unwrap()[..16] {
        assert_eq!(entry["status"], "pending");
        assert_eq!(entry["reason"], "channel_restore_unavailable");
    }
    stop(&mut f).await;
    for (id, saved) in records {
        write_sealed(&f, "eg_channel_records", &id, &saved, doc! {}).await;
    }
    let id = original["receipt_id"].as_str().unwrap();
    let mut sealed = read_sealed(&f, "eg_automation_records", id).await;
    for entry in sealed["value"]["dispatch"]["channels"]
        .as_array_mut()
        .unwrap()
    {
        entry["next_attempt_at"] = 0.into();
    }
    write_sealed(
        &f,
        "eg_automation_records",
        id,
        &sealed,
        doc! {"next_dispatch_at":0_i64,"lease_until":0_i64},
    )
    .await;
    f.base = f.replica().await;
    let recovered = wait_receipt(&f, &app, all_dispatched).await;
    assert_eq!(recovered["task_ids"].as_array().unwrap().len(), 17);
    assert_eq!(identities(&recovered), tasks);
    assert_posts(&f, 17).await;
}
