use super::{bindings::*, *};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn retired_installation_can_be_approved_by_a_new_owner_without_losing_history() {
    replacement(false).await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn historical_installation_identity_does_not_block_a_replacement_integration() {
    replacement(true).await;
}

async fn replacement(historical_identity: bool) {
    let mut f = Fixture::new(true).await;
    f.authoring = true;
    f.base = f.replica().await;
    let (old_app, mut old_binding) = pending(&f).await;
    if historical_identity {
        retain_historical_identity(&f, &mut old_binding).await;
    }
    let old_ref = old_app["app_ref"].as_str().unwrap();
    let old_id = old_binding["binding_id"].as_str().unwrap();
    let body = json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},"issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7"},"sender":{"login":"octocat","type":"User"}}).to_string();
    let old_url = format!("{}/v1/github/apps/{old_ref}/webhooks", f.base);
    let receipt = f
        .webhook(
            &old_url,
            "fixture-webhook-secret",
            "before-retirement",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(receipt.status(), 200);
    let receipt: serde_json::Value = receipt.json().await.unwrap();
    let approved = verification(&f, &old_binding, "token-alice", "approve-old")
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), 200);
    let approved: serde_json::Value = approved.json().await.unwrap();
    let mut channel = create_body();
    channel["sources"] = json!([{"type":"github","app_ref":old_ref,"installation_ids":[99],"repository_ids":[1001],"events":["issues"],"actions":["opened"]}]);
    channel["partition"] = json!({"version":1,"key":"fixed"});
    channel["reply"]["auto_final"] = false.into();
    channel["reply"]["sinks"] = json!([]);
    let created = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-alice")
        .json(&channel)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let channel: serde_json::Value = created.json().await.unwrap();
    let source_path = format!(
        "/v1/channels/{}/sources/0/test",
        channel["channel_id"].as_str().unwrap()
    );
    assert_authority(&f, &source_path, "passed").await;
    let retirement = f
        .client
        .post(format!("{}/v1/github/apps/{old_ref}/retirement", f.base))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "retire-old")
        .json(&json!({"expected_version":old_app["version"]}));
    assert_eq!(retirement.send().await.unwrap().status(), 200);
    assert_authority(&f, &source_path, "unavailable").await;

    let new_app = f
        .register("token-bob", "replacement", Some("fixture-webhook-secret"))
        .await;
    let new_ref = new_app["app_ref"].as_str().unwrap();
    let verification_url = format!("{}/v1/github/apps/{new_ref}/verification", f.base);
    assert_eq!(
        f.client
            .post(verification_url)
            .bearer_auth("token-bob")
            .header("Idempotency-Key", "approve-replacement")
            .json(&json!({"expected_version":1,"evidence_ref":"review:replacement"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // A historical binding must not turn an unbound replacement webhook into 404.
    let unbound = f
        .webhook(
            &format!("{}/v1/github/apps/{new_ref}/webhooks", f.base),
            "fixture-webhook-secret",
            "replacement-unbound",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(unbound.status(), 200);
    assert_eq!(
        unbound.json::<serde_json::Value>().await.unwrap()["reason"],
        "installation_unbound"
    );
    let request = json!({"app_ref":new_ref,"installation_id":99,"account_id":123,"repository_ids":[1001,1002]});
    let new_binding = f
        .client
        .post(format!("{}/v1/github/installation-bindings", f.base))
        .bearer_auth("token-bob")
        .header("Idempotency-Key", "replacement-binding")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(new_binding.status(), 201);
    let new_binding: serde_json::Value = new_binding.json().await.unwrap();
    assert_ne!(new_binding["binding_id"], old_id);
    assert_eq!(new_binding["status"], "pending_verification");
    assert_eq!(new_binding["repository_ids"], json!([]));
    let new_url = format!("{}/v1/github/apps/{new_ref}/webhooks", f.base);
    let pending_receipt = f
        .webhook(
            &new_url,
            "fixture-webhook-secret",
            "replacement-pending",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(pending_receipt.status(), 200);
    assert_eq!(
        pending_receipt.json::<serde_json::Value>().await.unwrap()["reason"],
        "installation_pending_verification"
    );
    assert_eq!(
        verification(&f, &new_binding, "token-bob", "approve-new")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    // Approval reaches the receipt-first boundary; no channel is configured in
    // this authority-only fixture, so the receipt is terminally ignored.
    let business = f
        .webhook(
            &format!("{}/v1/github/apps/{new_ref}/webhooks", f.base),
            "fixture-webhook-secret",
            "replacement-business",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(business.status(), 200);
    assert_eq!(
        business.json::<serde_json::Value>().await.unwrap()["reason"],
        "no_matching_channel"
    );
    assert_eq!(
        f.webhook(
            &format!("{}/v1/github/apps/{old_ref}/webhooks", f.base),
            "fixture-webhook-secret",
            "before-retirement",
            "issues",
            &body
        )
        .send()
        .await
        .unwrap()
        .status(),
        410
    );

    let history = [
        (
            format!("/v1/github/installation-bindings/{old_id}"),
            approved,
        ),
        (
            format!("/v1/github/apps/{old_ref}/deliveries/before-retirement"),
            receipt,
        ),
    ];
    for (path, expected) in history {
        let read = f.read(&f.base, &path, "token-alice").await;
        assert_eq!(read.status(), 200);
        assert_eq!(read.json::<serde_json::Value>().await.unwrap(), expected);
        assert_eq!(f.read(&f.base, &path, "token-bob").await.status(), 404);
    }
    let new_history = format!("/v1/github/apps/{new_ref}/deliveries/replacement-pending");
    assert_eq!(
        f.read(&f.base, &new_history, "token-bob").await.status(),
        200
    );
    assert_eq!(
        f.read(&f.base, &new_history, "token-alice").await.status(),
        404
    );
    // Exact old idempotency keys must not resurrect their saved successful responses.
    assert_eq!(
        verification(&f, &old_binding, "token-alice", "approve-old")
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    assert_eq!(f.client.post(format!("{}/v1/github/installation-bindings", f.base))
        .bearer_auth("token-alice").header("Idempotency-Key", "binding")
        .json(&json!({"app_ref":old_ref,"installation_id":99,"account_id":123,"repository_ids":[1001,1002]}))
        .send().await.unwrap().status(), 410);
    assert_eq!(
        f.client
            .post(format!("{}/v1/github/apps/{old_ref}/verification", f.base))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "verify-app")
            .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
}

async fn assert_authority(f: &Fixture, path: &str, expected: &str) {
    let checked = f
        .client
        .post(format!("{}{path}", f.base))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(checked.status(), 200);
    let checked: serde_json::Value = checked.json().await.unwrap();
    for id in [
        "github_integration",
        "github_installation_99",
        "github_repository_authority",
    ] {
        assert_eq!(
            checked["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|check| check["id"] == id)
                .unwrap()["status"],
            expected
        );
    }
}

async fn retain_historical_identity(f: &Fixture, binding: &mut serde_json::Value) {
    use mongodb::bson::{doc, spec::BinarySubtype, Binary};
    use sha2::{Digest, Sha256};
    let old_id = binding["binding_id"].as_str().unwrap().to_owned();
    let input = serde_json::to_vec(&("github.com", "binding", (42_u64, 99_u64))).unwrap();
    let id = format!("binding_{:x}", Sha256::digest(input));
    binding["binding_id"] = id.clone().into();
    let records = f
        .database
        .admin
        .database(&f.database.name)
        .collection::<mongodb::bson::Document>("eg_automation_records");
    let mut row = records
        .find_one(doc! {"_id":&old_id})
        .await
        .unwrap()
        .unwrap();
    row.insert("_id", &id);
    let sealed = cmaeg_core::seal::seal_blob(
        &key_ring(1),
        &serde_json::to_vec(&json!({"id":id,"owner":"alice","kind":"binding","value":binding}))
            .unwrap(),
    )
    .unwrap();
    row.insert(
        "blob",
        Binary {
            subtype: BinarySubtype::Generic,
            bytes: sealed,
        },
    );
    records.delete_one(doc! {"_id":old_id}).await.unwrap();
    records.insert_one(row).await.unwrap();
}
