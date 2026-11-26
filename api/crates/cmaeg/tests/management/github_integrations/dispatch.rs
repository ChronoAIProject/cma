#[path = "dispatch_restart.rs"]
mod restart;
#[path = "dispatch_restore.rs"]
mod restore;

use super::{
    bindings::{pending, verification},
    sources::{channel, issue_event, ready_dispatch_fixture},
    *,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use mongodb::bson::{doc, Binary, Document};
use tokio::time::{sleep, timeout, Duration};

fn webhook_url(f: &Fixture, app: &serde_json::Value) -> String {
    format!(
        "{}/v1/github/apps/{}/webhooks",
        f.base,
        app["app_ref"].as_str().unwrap()
    )
}

fn response_posts(f: &Fixture) -> Vec<super::super::fake_provider::ReceivedRequest> {
    f.provider
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/v2/responses")
        .collect()
}

async fn wait_for_posts(f: &Fixture, expected: usize) {
    timeout(Duration::from_secs(20), async {
        loop {
            if response_posts(f).len() >= expected {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("provider receives the expected number of ordinary turns");
}

async fn block_first_dispatch(f: &mut Fixture, label: &str) {
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let application = format!("github-dispatch-{label}-{}", uuid::Uuid::new_v4());
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    f.database
        .admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["update"],"appName":&application,"blockConnection":true,"blockTimeMS":900}
        })
        .await
        .unwrap();
}

async fn pending_webhook(
    f: &Fixture,
    app: &serde_json::Value,
    delivery_id: &str,
) -> (
    tokio::task::JoinHandle<reqwest::Result<reqwest::Response>>,
    String,
) {
    let body = issue_event();
    let request = f
        .webhook(
            &webhook_url(f, app),
            "fixture-webhook-secret",
            delivery_id,
            "issues",
            &body,
        )
        .send();
    let handle = tokio::spawn(request);
    sleep(Duration::from_millis(150)).await;
    (handle, body)
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn authority_races_uninstall_exclusion_and_retirement_refuse_without_provider_post() {
    for scenario in ["uninstall", "exclusion", "retirement"] {
        let (mut f, app, _channel_id) = ready_dispatch_fixture().await;
        block_first_dispatch(&mut f, scenario).await;
        let before_posts = response_posts(&f).len();
        let (webhook, body) = pending_webhook(&f, &app, &format!("race-{scenario}")).await;

        match scenario {
            "uninstall" => {
                let event = json!({
                    "action":"deleted",
                    "installation":{"id":99,"app_id":42,"account":{"id":123}}
                });
                assert_eq!(
                    f.webhook(
                        &webhook_url(&f, &app),
                        "fixture-webhook-secret",
                        "race-uninstall-lifecycle",
                        "installation",
                        &event.to_string(),
                    )
                    .send()
                    .await
                    .unwrap()
                    .status(),
                    200
                );
            }
            "exclusion" => {
                let event = json!({
                    "action":"removed",
                    "installation":{"id":99,"app_id":42,"account":{"id":123}},
                    "repositories_removed":[{"id":1001}],
                    "repositories_added":[]
                });
                assert_eq!(
                    f.webhook(
                        &webhook_url(&f, &app),
                        "fixture-webhook-secret",
                        "race-exclusion-lifecycle",
                        "installation_repositories",
                        &event.to_string(),
                    )
                    .send()
                    .await
                    .unwrap()
                    .status(),
                    200
                );
            }
            "retirement" => {
                let current: serde_json::Value = f
                    .read(
                        &f.base,
                        &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
                        "token-alice",
                    )
                    .await
                    .json()
                    .await
                    .unwrap();
                let retirement = f
                    .client
                    .post(format!(
                        "{}/v1/github/apps/{}/retirement",
                        f.base,
                        app["app_ref"].as_str().unwrap()
                    ))
                    .bearer_auth("token-alice")
                    .header("Idempotency-Key", "race-retirement")
                    .json(&json!({"expected_version":current["version"]}))
                    .send()
                    .await
                    .unwrap();
                let retirement_status = retirement.status();
                let retirement_body = retirement.text().await.unwrap();
                assert_eq!(retirement_status, 200, "retirement body={retirement_body}");
            }
            _ => unreachable!(),
        }

        let first = timeout(Duration::from_secs(5), webhook)
            .await
            .expect("blocked dispatch eventually settles")
            .unwrap()
            .unwrap();
        assert!(first.status().is_success(), "scenario={scenario}");
        let _first_receipt: serde_json::Value = first.json().await.unwrap();
        if scenario == "retirement" {
            for child in &mut f.children {
                child.kill().await.unwrap();
            }
            f.base = f.replica().await;
        }
        let mut receipt = serde_json::Value::Null;
        for _ in 0..30 {
            sleep(Duration::from_secs(1)).await;
            receipt = if scenario == "retirement" {
                let lookup = f
                    .read(
                        &f.base,
                        &format!(
                            "/v1/github/apps/{}/deliveries/race-retirement",
                            app["app_ref"].as_str().unwrap()
                        ),
                        "token-alice",
                    )
                    .await;
                assert_eq!(lookup.status(), 200, "scenario={scenario}");
                lookup.json().await.unwrap()
            } else {
                let retry = f
                    .webhook(
                        &webhook_url(&f, &app),
                        "fixture-webhook-secret",
                        &format!("race-{scenario}"),
                        "issues",
                        &body,
                    )
                    .send()
                    .await
                    .unwrap();
                assert_eq!(retry.status(), 200, "scenario={scenario}");
                retry.json().await.unwrap()
            };
            if receipt["dispatch"][0]["status"] != "pending" {
                break;
            }
        }
        assert_eq!(receipt["task_ids"], json!([]), "scenario={scenario}");
        assert_eq!(
            receipt["dispatch"][0]["status"], "refused",
            "scenario={scenario} receipt={receipt}"
        );
        assert_eq!(
            response_posts(&f).len(),
            before_posts,
            "scenario={scenario}"
        );
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn unrelated_channel_edit_retries_guard_conflict_and_admits_once() {
    let (mut f, app, channel_id) = ready_dispatch_fixture().await;
    block_first_dispatch(&mut f, "channel-edit").await;
    let before_posts = response_posts(&f).len();
    let (webhook, body) = pending_webhook(&f, &app, "race-channel-edit").await;
    let current: serde_json::Value = f
        .read(
            &f.base,
            &format!("/v1/channels/{channel_id}"),
            "token-alice",
        )
        .await
        .json()
        .await
        .unwrap();
    let mut update = channel(&app);
    update["name"] = "display-name-only-edit".into();
    update["expected_version"] = current["version"].clone();
    assert_eq!(
        f.client
            .put(format!("{}/v1/channels/{channel_id}", f.base))
            .bearer_auth("token-alice")
            .json(&update)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let _ = timeout(Duration::from_secs(5), webhook)
        .await
        .expect("guard conflict request settles")
        .unwrap()
        .unwrap();
    let mut receipt = serde_json::Value::Null;
    for _ in 0..30 {
        sleep(Duration::from_secs(1)).await;
        let retry = f
            .webhook(
                &webhook_url(&f, &app),
                "fixture-webhook-secret",
                "race-channel-edit",
                "issues",
                &body,
            )
            .send()
            .await
            .unwrap();
        if retry.status() == 200 {
            receipt = retry.json().await.unwrap();
            if receipt["task_ids"]
                .as_array()
                .is_some_and(|ids| !ids.is_empty())
            {
                break;
            }
        }
    }
    assert_eq!(
        receipt["task_ids"].as_array().unwrap().len(),
        1,
        "receipt={receipt}"
    );
    wait_for_posts(&f, before_posts + 1).await;
    assert_eq!(response_posts(&f).len(), before_posts + 1);
    assert_eq!(receipt["dispatch"][0]["status"], "dispatched");
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn uncertain_first_admission_recovery_attaches_same_id_and_posts_identical_candidate_once() {
    let (mut f, app, _channel_id) = ready_dispatch_fixture().await;
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let application = format!("github-uncertain-{}", uuid::Uuid::new_v4());
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    f.database
        .admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand", "mode":{"times":1},
            "data":{"failCommands":["commitTransaction"],"appName":&application,"closeConnection":true,"errorCode":91_i32,"errorLabels":["UnknownTransactionCommitResult"]}
        })
        .await
        .unwrap();
    let body = issue_event();
    let first = f
        .webhook(
            &webhook_url(&f, &app),
            "fixture-webhook-secret",
            "uncertain-admission",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    let first_json = first.json::<serde_json::Value>().await.unwrap_or_default();
    let first_id = first_json["task_ids"]
        .as_array()
        .and_then(|ids| ids.first())
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut final_receipt = first_json;
    for attempt in 0..12 {
        if final_receipt["task_ids"]
            .as_array()
            .is_some_and(|ids| !ids.is_empty())
        {
            break;
        }
        sleep(Duration::from_millis(500)).await;
        let retry = f
            .webhook(
                &webhook_url(&f, &app),
                "fixture-webhook-secret",
                "uncertain-admission",
                "issues",
                &body,
            )
            .send()
            .await
            .unwrap();
        assert!(retry.status().is_success(), "attempt={attempt}");
        final_receipt = retry.json().await.unwrap();
    }
    let final_id = final_receipt["task_ids"][0].as_str().unwrap();
    if let Some(first_id) = first_id {
        assert_eq!(first_id, final_id);
    }
    wait_for_posts(&f, 1).await;
    let posts = response_posts(&f);
    assert_eq!(posts.len(), 1);
    assert!(!posts[0].body.contains("title-sentinel"));
    assert!(!posts[0].body.contains("body-sentinel"));
}

async fn owner_app_and_channel(
    f: &mut Fixture,
    token: &str,
    suffix: &str,
) -> (serde_json::Value, String) {
    let app_id = match suffix {
        "alice" => 101,
        "bob" => 102,
        "carol" => 103,
        _ => 104,
    };
    let app_response = f
        .registration(
            &f.base,
            token,
            &format!("fair-app-{suffix}"),
            &json!({"app_id":app_id,"display_name":format!("fair-{suffix}"),"webhook_secret":"fixture-webhook-secret"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(app_response.status(), 201);
    let app: serde_json::Value = app_response.json().await.unwrap();
    let verified = f
        .client
        .post(format!(
            "{}/v1/github/apps/{}/verification",
            f.base,
            app["app_ref"].as_str().unwrap()
        ))
        .bearer_auth(token)
        .header("Idempotency-Key", format!("verify-{suffix}"))
        .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(verified.status(), 200);
    let verified: serde_json::Value = verified.json().await.unwrap();
    let binding = f
        .client
        .post(format!("{}/v1/github/installation-bindings", f.base))
        .bearer_auth(token)
        .header("Idempotency-Key", format!("binding-{suffix}"))
        .json(&json!({"app_ref":verified["app_ref"],"installation_id":99,"account_id":123,"repository_ids":[1001]}))
        .send()
        .await
        .unwrap();
    assert_eq!(binding.status(), 201);
    let binding: serde_json::Value = binding.json().await.unwrap();
    let approved = f
        .client
        .post(format!(
            "{}/v1/github/installation-bindings/{}/verification",
            f.base,
            binding["binding_id"].as_str().unwrap()
        ))
        .bearer_auth(token)
        .header("Idempotency-Key", format!("approve-{suffix}"))
        .json(&json!({"expected_version":binding["version"],"evidence_ref":"review:installation-owner","repository_ids":[1001]}))
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), 200);
    let mut body = channel(&verified);
    body["profile"]["metadata"]["owner"]["subject"] = suffix.into();
    body["name"] = format!("fair-channel-{suffix}").into();
    let created = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let created_status = created.status();
    let created: serde_json::Value = created.json().await.unwrap();
    assert_eq!(created_status, 201, "channel creation={created}");
    (verified, created["channel_id"].as_str().unwrap().to_owned())
}

async fn rewrite_pending_receipt(f: &Fixture, receipt_id: &str, next_dispatch_at: i64) {
    let ring = cmaeg_core::seal::MasterKeyRing::parse(&format!(
        "1:{}",
        STANDARD_NO_PAD.encode([1_u8; 32])
    ))
    .unwrap();
    let collection = f
        .database
        .admin
        .database(&f.database.name)
        .collection::<Document>("eg_automation_records");
    let row = collection
        .find_one(doc! {"_id":&receipt_id})
        .await
        .unwrap()
        .unwrap();
    let blob = row.get_binary_generic("blob").unwrap();
    let opened = cmaeg_core::seal::open_blob(&ring, blob).unwrap();
    let mut sealed: serde_json::Value = serde_json::from_slice(&opened.payload).unwrap();
    sealed["value"]["receipt"]["status"] = "pending".into();
    sealed["value"]["receipt"]["task_ids"] = json!([]);
    if let Some(channels) = sealed["value"]["dispatch"]["channels"].as_array_mut() {
        for channel in channels {
            channel["status"] = "pending".into();
            channel["admission_id"] = serde_json::Value::Null;
            channel["attempts"] = 0.into();
            channel["next_attempt_at"] = next_dispatch_at.into();
        }
    }
    let resealed =
        cmaeg_core::seal::seal_blob(&ring, &serde_json::to_vec(&sealed).unwrap()).unwrap();
    collection
        .update_one(
            doc! {"_id":receipt_id},
            doc! {"$set":{"blob":Binary{subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:resealed},"next_dispatch_at":next_dispatch_at},"$unset":{"lease_until":""}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn recovery_fairness_pages_three_owners_and_future_attempts_without_starvation() {
    let mut f = Fixture::new(true).await;
    f.authoring = true;
    f.base = f.replica().await;
    let third = f.nyxid.oauth.issue_tokens("carol").0;
    let owners = [
        ("token-alice", "alice"),
        ("token-bob", "bob"),
        (&third, "carol"),
    ];
    let mut apps = Vec::new();
    for (token, name) in owners {
        apps.push((
            token.to_owned(),
            owner_app_and_channel(&mut f, token, name).await,
        ));
    }
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut receipts = Vec::new();
    for (owner_index, (_token, (app, _channel_id))) in apps.iter().enumerate() {
        let url = webhook_url(&f, app);
        for delivery_index in 0..7 {
            let delivery_id = format!("fair-{owner_index}-{delivery_index}");
            let response = f
                .webhook(
                    &url,
                    "fixture-webhook-secret",
                    &delivery_id,
                    "issues",
                    &issue_event(),
                )
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success());
            let receipt: serde_json::Value = response.json().await.unwrap();
            receipts.push(receipt["receipt_id"].as_str().unwrap().to_owned());
        }
    }
    for (index, receipt_id) in receipts.iter().enumerate() {
        rewrite_pending_receipt(&f, receipt_id, now + if index % 3 == 0 { 1 } else { 0 }).await;
    }
    for _tick in 0..5 {
        sleep(Duration::from_secs(5)).await;
        let mut complete = true;
        for (token, (app, _)) in &apps {
            for delivery_index in 0..7 {
                let receipt = f
                    .read(
                        &f.base,
                        &format!(
                            "/v1/github/apps/{}/deliveries/fair-{}-{}",
                            app["app_ref"].as_str().unwrap(),
                            apps.iter()
                                .position(|(candidate, _)| candidate == token)
                                .unwrap(),
                            delivery_index
                        ),
                        token,
                    )
                    .await;
                if receipt.status() != 200 {
                    complete = false;
                    continue;
                }
                let value: serde_json::Value = receipt.json().await.unwrap();
                if value["status"] == "pending" {
                    complete = false;
                }
            }
        }
        if complete {
            break;
        }
    }
    for receipt_id in receipts {
        let row = f
            .database
            .admin
            .database(&f.database.name)
            .collection::<Document>("eg_automation_records")
            .find_one(doc! {"_id":&receipt_id})
            .await
            .unwrap();
        assert!(row.is_some(), "receipt {receipt_id} was not retained");
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn task_get_preserves_completion_before_eviction_after_restart() {
    let (mut f, app, _channel_id) = ready_dispatch_fixture().await;
    let first = f
        .webhook(
            &webhook_url(&f, &app),
            "fixture-webhook-secret",
            "0000-task",
            "issues",
            &issue_event(),
        )
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let task_id = first["task_ids"][0].as_str().unwrap().to_owned();
    let task_path = format!("/v1/github/tasks/{task_id}");
    wait_for_posts(&f, 1).await;
    let before_eviction: serde_json::Value = f
        .read(&f.base, &task_path, "token-alice")
        .await
        .json()
        .await
        .unwrap();
    assert!(matches!(
        before_eviction["status"].as_str(),
        Some("accepted" | "running" | "succeeded")
    ));
    for index in 1..=51 {
        let response = f
            .webhook(
                &webhook_url(&f, &app),
                "fixture-webhook-secret",
                &format!("{index:04}-task"),
                "issues",
                &issue_event(),
            )
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
    let after_eviction: serde_json::Value = f
        .read(&f.base, &task_path, "token-alice")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after_eviction["status"], before_eviction["status"]);
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    let after_restart: serde_json::Value = f
        .read(&f.base, &task_path, "token-alice")
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(after_restart["status"], before_eviction["status"]);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn shared_agent_github_and_custom_channels_use_ordinary_routing_and_compact_projection() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    verification(&f, &binding, "token-alice", "approve-shared")
        .send()
        .await
        .unwrap();
    f.authoring = true;
    f.base = f.replica().await;
    let mut github = channel(&app);
    github["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_shared"});
    github["name"] = "shared-github".into();
    let github = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-alice")
        .json(&github)
        .send()
        .await
        .unwrap();
    assert_eq!(github.status(), 201);
    let github: serde_json::Value = github.json().await.unwrap();
    let github = super::sources::enable_routing(
        &f,
        github["channel_id"].as_str().unwrap(),
        &github["version"],
    )
    .await;
    let mut custom = super::super::create_body();
    custom["name"] = "shared-custom".into();
    custom["provider"]["target"] = json!({"type":"existing_agent","agent_id":"agt_shared"});
    custom["sources"] = json!([{"type":"custom","auth":{"type":"hmac","key_id":"h1"}}]);
    custom["partition"] = json!({"version":1,"key":"per_event"});
    custom["durable_custom_execution"] = true.into();
    custom["reply"]["sinks"] = json!([]);
    let custom = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-alice")
        .json(&custom)
        .send()
        .await
        .unwrap();
    assert_eq!(custom.status(), 201);
    let custom: serde_json::Value = custom.json().await.unwrap();
    f.routed_records
        .push(github["record_id"].as_str().unwrap().to_owned());
    f.routed_records
        .push(custom["record_id"].as_str().unwrap().to_owned());
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    let github_body = issue_event().replace(
        "\"state\":\"open\"",
        "\"state\":\"open\",\"title\":\"title-sentinel\",\"body\":\"body-sentinel\"",
    );
    let github_receipt = f
        .webhook(
            &webhook_url(&f, &app),
            "fixture-webhook-secret",
            "shared-github-event",
            "issues",
            &github_body,
        )
        .send()
        .await
        .unwrap();
    assert!(matches!(github_receipt.status().as_u16(), 200 | 202));
    let custom_event = json!({"specversion":"1.0","id":"shared-custom-event","type":"sync.requested","source":"https://chrono.example","subject":"board","data":{"sentinel":"custom-data"}}).to_string();
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
    let signature = cmaeg::auth::webhook::signature(
        &cmaeg_core::channel::Secret::new("aG1hYy1zZWNyZXQ="),
        "shared-custom-event",
        &timestamp,
        custom_event.as_bytes(),
    );
    let custom_response = f
        .client
        .post(format!(
            "{}/v1/channels/{}/events",
            f.base,
            custom["channel_id"].as_str().unwrap()
        ))
        .header("Content-Type", "application/cloudevents+json")
        .header("webhook-id", "shared-custom-event")
        .header("webhook-timestamp", timestamp)
        .header("webhook-signature", signature)
        .body(custom_event)
        .send()
        .await
        .unwrap();
    assert_eq!(custom_response.status(), 202);
    wait_for_posts(&f, 2).await;
    let posts = response_posts(&f);
    assert_eq!(posts.len(), 2);
    let github_post = posts
        .iter()
        .find(|request| request.body.contains("shared-github-event"))
        .expect("GitHub turn reaches the shared Agent");
    assert!(github_post.body.contains("fixture/source"));
    assert!(!github_post.body.contains("title-sentinel"));
    assert!(!github_post.body.contains("body-sentinel"));
    assert!(posts
        .iter()
        .any(|request| request.body.contains("shared-custom-event")));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn transient_channel_restore_during_dispatch_retries_then_admits_once() {
    restore::assert_channel_restore_retry().await;
}
