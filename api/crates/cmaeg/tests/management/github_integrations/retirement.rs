use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn retired_personal_draft_refuses_all_webhooks_and_mutations_but_keeps_receipts() {
    let mut f = Fixture::new(true).await;
    let app = f
        .register(
            "token-alice",
            "personal-draft",
            Some("fixture-webhook-secret"),
        )
        .await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let url = format!("{}/v1/github/apps/{app_ref}", f.base);
    let ping = json!({"zen":"fixture","hook_id":1,"hook":{"type":"App"}}).to_string();
    let retained = f
        .webhook(
            &format!("{url}/webhooks"),
            "fixture-webhook-secret",
            "ping-before",
            "ping",
            &ping,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(retained.status(), 200);
    let retained: serde_json::Value = retained.json().await.unwrap();
    let retire = |token: &str, version: u64, key: &str| {
        f.client
            .post(format!("{url}/retirement"))
            .bearer_auth(token)
            .header("Idempotency-Key", key)
            .json(&json!({"expected_version":version}))
    };
    assert_eq!(
        retire("token-bob", 1, "retire")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        retire("token-alice", 2, "wrong-version")
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
    let response = retire("token-alice", 1, "retire").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let retired: serde_json::Value = response.json().await.unwrap();
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["version"], 2);
    let replay = retire("token-alice", 1, "retire").send().await.unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), retired);
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    let url = format!("{}/v1/github/apps/{app_ref}", f.base);
    for id in ["ping-before", "ping-after"] {
        let response = f
            .webhook(
                &format!("{url}/webhooks"),
                "fixture-webhook-secret",
                id,
                "ping",
                &ping,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 410);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "github_integration_retired"
        );
    }
    assert_eq!(
        f.read(
            &f.base,
            &format!("/v1/github/apps/{app_ref}/deliveries/ping-after"),
            "token-alice"
        )
        .await
        .status(),
        404
    );
    let found = f
        .read(
            &f.base,
            &format!("/v1/github/apps/{app_ref}/deliveries/ping-before"),
            "token-alice",
        )
        .await;
    assert_eq!(found.status(), 200);
    assert_eq!(found.json::<serde_json::Value>().await.unwrap(), retained);
    for (path, method, body) in [
        (
            format!("{url}/verification"),
            reqwest::Method::POST,
            json!({"expected_version":2,"evidence_ref":"review:retired"}),
        ),
        (
            format!("{url}/webhook-secret"),
            reqwest::Method::PUT,
            json!({"expected_version":2,"webhook_secret":"fixture-replacement-secret"}),
        ),
        (
            format!("{}/v1/github/installation-bindings", f.base),
            reqwest::Method::POST,
            json!({"app_ref":app_ref,"installation_id":99,"account_id":123,"repository_ids":[1001]}),
        ),
    ] {
        let response = f
            .client
            .request(method, path)
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "after-retirement")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 410);
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn retirement_fences_an_approval_delayed_by_external_storage() {
    use mongodb::bson::doc;
    let mut f = Fixture::new(true).await;
    let (app, binding) = super::bindings::pending(&f).await;
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let application = format!("approval-race-{}", uuid::Uuid::new_v4());
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    // Delay only this disposable gateway's first update at the external Mongo boundary.
    f.database.admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand", "mode":{"times":1},
        "data":{"failCommands":["update"],"appName":&application,"blockConnection":true,"blockTimeMS":800}
    }).await.unwrap();
    let approval =
        super::bindings::verification(&f, &binding, "token-alice", "contended-approval").send();
    tokio::pin!(approval);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut approval)
            .await
            .is_err()
    );
    let retired = f
        .client
        .post(format!(
            "{}/v1/github/apps/{}/retirement",
            f.base,
            app["app_ref"].as_str().unwrap()
        ))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "retire")
        .json(&json!({"expected_version":app["version"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(retired.status(), 200);
    let response = approval.await.unwrap();
    assert!(!response.status().is_success());
    let read = f
        .read(
            &f.base,
            &format!(
                "/v1/github/installation-bindings/{}",
                binding["binding_id"].as_str().unwrap()
            ),
            "token-alice",
        )
        .await;
    assert_eq!(read.status(), 200);
    assert_eq!(read.json::<serde_json::Value>().await.unwrap(), binding);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn retiring_a_verified_app_releases_its_identity_for_another_owners_verification() {
    let f = Fixture::new(true).await;
    let (app, _binding) = super::bindings::pending(&f).await;
    let other = f.register("token-bob", "replacement-app", None).await;
    let other_ref = other["app_ref"].as_str().unwrap();
    let verify = || {
        f.client
            .post(format!(
                "{}/v1/github/apps/{other_ref}/verification",
                f.base
            ))
            .bearer_auth("token-bob")
            .header("Idempotency-Key", "verify-replacement")
            .json(&json!({"expected_version":1,"evidence_ref":"review:new-owner"}))
    };
    assert_eq!(verify().send().await.unwrap().status(), 409);
    let old_ref = app["app_ref"].as_str().unwrap();
    let retire = || {
        f.client
            .post(format!("{}/v1/github/apps/{old_ref}/retirement", f.base))
            .bearer_auth("token-alice")
            .header("Idempotency-Key", "retire-verified")
            .json(&json!({"expected_version":app["version"]}))
    };
    assert_eq!(retire().send().await.unwrap().status(), 200);
    let response = verify().send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["status"],
        "verified"
    );
    assert_eq!(retire().send().await.unwrap().status(), 200);
    let replay = f
        .client
        .post(format!("{}/v1/github/apps/{old_ref}/verification", f.base))
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "verify-app")
        .json(&json!({"expected_version":1,"evidence_ref":"review:app"}))
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 410);
    assert_eq!(verify().send().await.unwrap().status(), 200);
}
