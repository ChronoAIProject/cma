use super::{
    bindings::{pending, verification},
    *,
};

async fn binding_read(f: &Fixture, binding: &serde_json::Value) -> serde_json::Value {
    let response = f
        .read(
            &f.base,
            &format!(
                "/v1/github/installation-bindings/{}",
                binding["binding_id"].as_str().unwrap()
            ),
            "token-alice",
        )
        .await;
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}
async fn lifecycle(
    f: &Fixture,
    app: &serde_json::Value,
    id: &str,
    event: &str,
    body: serde_json::Value,
) -> serde_json::Value {
    let response = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            id,
            event,
            &body.to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}
fn installation(action: &str) -> serde_json::Value {
    json!({"action":action,"installation":{"id":99,"app_id":42,"account":{"id":123}}})
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn suspension_survives_restart_and_delayed_positive_events_never_restore_authority() {
    let mut f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let suspended = lifecycle(&f, &app, "suspend", "installation", installation("suspend")).await;
    assert_eq!(suspended["authority_revision"], 2);
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["status"], "suspended");
    assert_eq!(binding["version"], 3);
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    f.base = f.replica().await;
    assert_eq!(
        lifecycle(&f, &app, "suspend", "installation", installation("suspend")).await,
        suspended
    );
    for action in ["created", "unsuspend", "new_permissions_accepted"] {
        lifecycle(&f, &app, action, "installation", installation(action)).await;
        assert_eq!(binding_read(&f, &pending).await, binding);
    }
    let replay = verification(&f, &pending, "token-alice", "approve")
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(binding_read(&f, &pending).await, binding);
    let approved = verification(&f, &binding, "token-alice", "reapprove")
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), 200);
    assert_eq!(
        approved.json::<serde_json::Value>().await.unwrap()["authority_revision"],
        3
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn repository_removal_requires_explicit_reapproval_and_channel_selection_cannot_restore_it() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut removed = installation("removed");
    removed["repositories_removed"] = json!([{"id":1001}]);
    removed["repositories_added"] = json!([]);
    lifecycle(&f, &app, "removed", "installation_repositories", removed).await;
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["repository_ids"], json!([1002]));
    assert_eq!(binding["approved_repository_ids"], json!([1001, 1002]));
    assert_eq!(binding["excluded_repository_ids"], json!([1001]));
    assert_eq!(binding["authority_revision"], 2);
    let mut added = installation("added");
    added["repositories_removed"] = json!([]);
    added["repositories_added"] = json!([{"id":1001}]);
    lifecycle(&f, &app, "added", "installation_repositories", added).await;
    assert_eq!(binding_read(&f, &pending).await, binding);
    let response = verification(&f, &binding, "token-alice", "reapprove")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let approved: serde_json::Value = response.json().await.unwrap();
    assert_eq!(approved["repository_ids"], json!([1001, 1002]));
    assert_eq!(approved["excluded_repository_ids"], json!([]));
    assert_eq!(approved["authority_revision"], 3);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn business_owner_mismatch_durably_excludes_the_repository() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let body = json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"owner":{"id":456}},"issue":{"id":9001,"number":7}});
    let receipt = lifecycle(&f, &app, "owner-changed", "issues", body).await;
    assert_eq!(receipt["reason"], "repository_owner_changed");
    assert_eq!(receipt["authority_revision"], 2);
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["excluded_repository_ids"], json!([1001]));
    assert_eq!(binding["repository_ids"], json!([1002]));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn concurrent_repository_tightenings_are_both_retained() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let url = format!(
        "{}/v1/github/apps/{}/webhooks",
        f.base,
        app["app_ref"].as_str().unwrap()
    );
    let removed = |id| {
        json!({"action":"removed","installation":{"id":99,"app_id":42,"account":{"id":123}},"repositories_added":[],"repositories_removed":[{"id":id}]}).to_string()
    };
    let (first, second) = tokio::join!(
        f.webhook(
            &url,
            "fixture-webhook-secret",
            "remove-a",
            "installation_repositories",
            &removed(1001)
        )
        .send(),
        f.webhook(
            &url,
            "fixture-webhook-secret",
            "remove-b",
            "installation_repositories",
            &removed(1002)
        )
        .send(),
    );
    assert_eq!(first.unwrap().status(), 200);
    assert_eq!(second.unwrap().status(), 200);
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["excluded_repository_ids"], json!([1001, 1002]));
    assert_eq!(binding["repository_ids"], json!([]));
    assert_eq!(binding["authority_revision"], 3);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn repository_transfer_deletion_and_installation_uninstall_remain_tightened() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for (id, action) in [(1001, "transferred"), (1002, "deleted")] {
        lifecycle(&f,&app,action,"repository",json!({"action":action,"installation":{"id":99},"repository":{"id":id,"owner":{"id":123}}})).await;
    }
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["excluded_repository_ids"], json!([1001, 1002]));
    assert_eq!(binding["authority_revision"], 3);
    lifecycle(
        &f,
        &app,
        "uninstall",
        "installation",
        installation("deleted"),
    )
    .await;
    let uninstalled = binding_read(&f, &pending).await;
    assert_eq!(uninstalled["status"], "uninstalled");
    assert_eq!(uninstalled["authority_revision"], 4);
    for action in [
        "suspend",
        "created",
        "unsuspend",
        "new_permissions_accepted",
    ] {
        lifecycle(&f, &app, action, "installation", installation(action)).await;
        assert_eq!(binding_read(&f, &pending).await, uninstalled);
    }
    assert_eq!(
        verification(&f, &uninstalled, "token-alice", "cannot-reapprove")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn a_large_installation_removal_keeps_only_exclusions_in_the_binding_scope() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut repositories = vec![json!({"id":1001})];
    repositories.extend((2000..12000).map(|id| json!({"id":id})));
    let mut body = installation("removed");
    body["repositories_removed"] = json!(repositories);
    body["repositories_added"] = json!([]);
    assert!(body.to_string().len() < 256 * 1024);
    lifecycle(&f, &app, "large-removal", "installation_repositories", body).await;
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["repository_ids"], json!([1002]));
    assert_eq!(binding["excluded_repository_ids"], json!([1001]));
    assert_eq!(binding["authority_revision"], 2);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn outside_scope_removal_fences_an_older_request_to_expand_approval() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    let approved = verification(&f, &pending, "token-alice", "approve")
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap();
    let mut body = installation("removed");
    body["repositories_removed"] = json!([{"id":9999}]);
    body["repositories_added"] = json!([]);
    lifecycle(
        &f,
        &app,
        "outside-removal",
        "installation_repositories",
        body,
    )
    .await;
    let binding = binding_read(&f, &pending).await;
    assert_eq!(binding["repository_ids"], json!([1001, 1002]));
    assert_eq!(binding["excluded_repository_ids"], json!([]));
    assert_eq!(binding["authority_revision"], 1);
    assert_eq!(binding["version"], 3);
    let response=f.client.post(format!("{}/v1/github/installation-bindings/{}/verification",f.base,binding["binding_id"].as_str().unwrap()))
        .bearer_auth("token-alice").header("Idempotency-Key","stale-expansion")
        .json(&json!({"expected_version":approved["version"],"evidence_ref":"review:older-expansion","repository_ids":[1001,1002,9999]})).send().await.unwrap();
    assert_eq!(response.status(), 409);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn an_exclusion_repeated_under_a_new_delivery_id_does_not_revoke_authority_again() {
    let f = Fixture::new(true).await;
    let (app, pending) = pending(&f).await;
    assert_eq!(
        verification(&f, &pending, "token-alice", "approve")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let mut body = installation("removed");
    body["repositories_removed"] = json!([{"id":1001}]);
    body["repositories_added"] = json!([]);
    lifecycle(
        &f,
        &app,
        "first-removal",
        "installation_repositories",
        body.clone(),
    )
    .await;
    let first = binding_read(&f, &pending).await;
    lifecycle(
        &f,
        &app,
        "second-removal",
        "installation_repositories",
        body,
    )
    .await;
    let second = binding_read(&f, &pending).await;
    assert_eq!(first["authority_revision"], 2);
    assert_eq!(second["authority_revision"], 2);
    assert_eq!(second["repository_ids"], json!([1002]));
    assert_eq!(second["excluded_repository_ids"], json!([1001]));
}
