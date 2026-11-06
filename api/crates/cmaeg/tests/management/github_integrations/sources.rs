use super::{
    bindings::{pending, verification},
    *,
};

fn channel(app: &serde_json::Value) -> serde_json::Value {
    let mut body = create_body();
    body["sources"] = json!([{"type":"github","app_ref":app["app_ref"],"installation_ids":[99],"repository_ids":[1001],"events":["issues"],"actions":["opened"]}]);
    body["partition"] = json!({"version":1,"key":"fixed"});
    body["reply"]["auto_final"] = false.into();
    body["reply"]["sinks"] = json!([]);
    body
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn channel_source_cannot_reference_another_owners_integration() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    assert_eq!(
        verification(&f, &binding, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    f.authoring = true;
    f.base = f.replica().await;
    let mut body = channel(&app);
    body["profile"]["metadata"]["owner"]["subject"] = "bob".into();
    let response = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-bob")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let result: serde_json::Value = response.json().await.unwrap();
    assert_eq!(status, 404, "{result}");
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn channel_source_requires_approved_repository_authority_and_reports_live_status() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    f.authoring = true;
    f.base = f.replica().await;
    let body = channel(&app);
    let create = |body: &serde_json::Value| {
        f.client
            .post(format!("{}/v1/channels", f.base))
            .bearer_auth("token-alice")
            .json(body)
    };
    assert_eq!(create(&body).send().await.unwrap().status(), 400);
    assert_eq!(
        verification(&f, &binding, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut outside = body.clone();
    outside["sources"][0]["repository_ids"] = json!([9999]);
    assert_eq!(create(&outside).send().await.unwrap().status(), 400);
    let created = create(&body).send().await.unwrap();
    let status = created.status();
    let saved: serde_json::Value = created.json().await.unwrap();
    assert_eq!(status, 201, "{saved}");
    let channel_id = saved["channel_id"].as_str().unwrap();
    let test_url = format!("{}/v1/channels/{channel_id}/sources/0/test", f.base);
    let checked = f
        .client
        .post(&test_url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    assert_eq!(checked.status(), 200);
    let checked: serde_json::Value = checked.json().await.unwrap();
    let checks = checked["checks"].as_array().unwrap();
    for id in [
        "github_integration",
        "github_webhook_credential",
        "github_repository_authority",
    ] {
        assert_eq!(
            checks.iter().find(|check| check["id"] == id).unwrap()["status"],
            "passed"
        );
    }
    assert_eq!(
        checks
            .iter()
            .find(|check| check["id"] == "github_dispatch")
            .unwrap()["status"],
        "unavailable"
    );
    let suspend =
        json!({"action":"suspend","installation":{"id":99,"app_id":42,"account":{"id":123}}})
            .to_string();
    assert_eq!(
        f.webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            "suspend",
            "installation",
            &suspend
        )
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    let checked: serde_json::Value = f
        .client
        .post(test_url)
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        checked["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "github_installation_99")
            .unwrap()["status"],
        "unavailable"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn disabling_github_preserves_existing_sources_and_allows_their_removal() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    assert_eq!(
        verification(&f, &binding, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    f.authoring = true;
    f.base = f.replica().await;
    let mut body = channel(&app);
    let created = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let saved: serde_json::Value = created.json().await.unwrap();
    let disabled = f.replica_with_flag(Some(false)).await;
    let url = format!(
        "{disabled}/v1/channels/{}",
        saved["channel_id"].as_str().unwrap()
    );
    body.as_object_mut().unwrap().remove("credentials");
    body["expected_version"] = saved["version"].clone();
    body["name"] = "Retained source".into();
    let retained = f
        .client
        .put(&url)
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(retained.status(), 200);
    let retained: serde_json::Value = retained.json().await.unwrap();
    assert_eq!(
        retained["definition"]["sources"],
        saved["definition"]["sources"]
    );
    body["expected_version"] = retained["version"].clone();
    body["sources"][0]["repository_ids"] = json!([1002]);
    assert_eq!(
        f.client
            .put(&url)
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    body["sources"] = create_body()["sources"].clone();
    assert_eq!(
        f.client
            .put(&url)
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}
