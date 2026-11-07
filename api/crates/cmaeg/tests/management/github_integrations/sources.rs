use super::{
    bindings::{pending, verification},
    *,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn approved_delivery_creates_one_task_per_matching_channel_and_redelivery_is_idempotent() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    assert_eq!(
        verification(&f, &binding, "token-alice", "approve-dispatch")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    f.authoring = true;
    f.base = f.replica().await;
    for (index, name) in ["GitHub route one", "GitHub route two"]
        .into_iter()
        .enumerate()
    {
        let mut body = channel(&app);
        body["name"] = name.into();
        if index == 1 {
            body["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        let response = f
            .client
            .post(format!("{}/v1/channels", f.base))
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
    }
    let body = json!({
        "action":"opened",
        "installation":{"id":99},
        "repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},
        "issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7","state":"open"},
        "sender":{"login":"octocat","type":"User"}
    }).to_string();
    let url = format!(
        "{}/v1/github/apps/{}/webhooks",
        f.base,
        app["app_ref"].as_str().unwrap()
    );
    let first = f
        .webhook(
            &url,
            "fixture-webhook-secret",
            "dispatch-1",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 202);
    let first: serde_json::Value = first.json().await.unwrap();
    let task_ids = first["task_ids"].as_array().unwrap();
    assert_eq!(task_ids.len(), 2, "receipt={first}");
    assert_eq!(first["dispatch"].as_array().unwrap().len(), 2);
    for task_id in task_ids {
        let task = f
            .read(
                &f.base,
                &format!("/v1/github/tasks/{}", task_id.as_str().unwrap()),
                "token-alice",
            )
            .await;
        assert_eq!(task.status(), 200);
        assert_eq!(
            task.json::<serde_json::Value>().await.unwrap()["task_id"],
            *task_id
        );
    }
    let retry = f
        .webhook(
            &url,
            "fixture-webhook-secret",
            "dispatch-1",
            "issues",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), 200);
    assert_eq!(retry.json::<serde_json::Value>().await.unwrap(), first);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn filtered_action_is_ignored_without_a_task_and_free_text_never_reaches_provider() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    verification(&f, &binding, "token-alice", "approve-filter")
        .send()
        .await
        .unwrap();
    f.authoring = true;
    f.base = f.replica().await;
    let mut body = channel(&app);
    body["name"] = "GitHub filter route".into();
    let response = f
        .client
        .post(format!("{}/v1/channels", f.base))
        .bearer_auth("token-alice")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let mut payload = json!({
        "action":"closed",
        "installation":{"id":99},
        "repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},
        "issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7","state":"closed","title":"title-sentinel","body":"body-sentinel"},
        "sender":{"login":"octocat","type":"User"}
    }).to_string();
    let url = format!(
        "{}/v1/github/apps/{}/webhooks",
        f.base,
        app["app_ref"].as_str().unwrap()
    );
    let response = f
        .webhook(
            &url,
            "fixture-webhook-secret",
            "filtered-1",
            "issues",
            &payload,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(receipt["reason"], "no_matching_channel");
    assert_eq!(receipt["task_ids"], json!([]));
    payload = payload.replace("\"closed\"", "\"opened\"");
    let response = f
        .webhook(
            &url,
            "fixture-webhook-secret",
            "sentinel-1",
            "issues",
            &payload,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert!(f.provider.requests().iter().all(|request| {
        !request.body.contains("title-sentinel") && !request.body.contains("body-sentinel")
    }));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn thirty_two_mixed_deliveries_keep_distinct_receipts_and_owner_scope() {
    use futures_util::future::join_all;
    let f = Fixture::new(true).await;
    let (app, _binding) = pending(&f).await;
    let body = json!({
        "action":"closed",
        "installation":{"id":99},
        "repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},
        "issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7","state":"closed"},
        "sender":{"login":"octocat","type":"User"}
    }).to_string();
    let url = format!(
        "{}/v1/github/apps/{}/webhooks",
        f.base,
        app["app_ref"].as_str().unwrap()
    );
    let responses = join_all((0..32).map(|index| {
        f.webhook(
            &url,
            "fixture-webhook-secret",
            &format!("mixed-{index}"),
            "issues",
            &body,
        )
        .send()
    }))
    .await;
    for response in responses {
        assert_eq!(response.unwrap().status(), 200);
    }
    let read = f
        .read(
            &f.base,
            &format!(
                "/v1/github/apps/{}/deliveries/mixed-31",
                app["app_ref"].as_str().unwrap()
            ),
            "token-alice",
        )
        .await;
    assert_eq!(read.status(), 200);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn route_removed_before_dispatch_cannot_create_an_admission() {
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    verification(&f, &binding, "token-alice", "approve-route-race")
        .send()
        .await
        .unwrap();
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
    let saved = created.json::<serde_json::Value>().await.unwrap();
    let channel_url = format!(
        "{}/v1/channels/{}",
        f.base,
        saved["channel_id"].as_str().unwrap()
    );
    body["expected_version"] = saved["version"].clone();
    body["sources"][0]["repository_ids"] = json!([1002]);
    assert_eq!(
        f.client
            .put(&channel_url)
            .bearer_auth("token-alice")
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let event = json!({
        "action":"opened","installation":{"id":99},
        "repository":{"id":1001,"full_name":"fixture/source","owner":{"id":123}},
        "issue":{"id":9001,"number":7,"html_url":"https://github.com/fixture/source/issues/7","state":"open"},
        "sender":{"login":"octocat","type":"User"}
    }).to_string();
    let response = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            "route-race-1",
            "issues",
            &event,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(receipt["task_ids"], json!([]));
    assert_eq!(receipt["reason"], "no_matching_channel");
}

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
        "passed"
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
