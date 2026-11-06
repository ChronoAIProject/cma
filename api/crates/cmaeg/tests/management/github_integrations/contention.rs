use super::{
    bindings::{pending, verification},
    *,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn unrelated_receipts_do_not_contend_with_authority_and_lifecycle_retries_within_budget() {
    use mongodb::bson::doc;
    let f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    assert_eq!(
        verification(&f, &binding, "token-alice", "approve")
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
    let mut session = f.database.admin.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    let records = f
        .database
        .admin
        .database(&f.database.name)
        .collection::<mongodb::bson::Document>("eg_automation_records");
    for id in [
        app["app_ref"].as_str().unwrap(),
        binding["binding_id"].as_str().unwrap(),
    ] {
        records
            .update_one(doc! {"_id":id}, doc! {"$inc":{"version":1_i64}})
            .session(&mut session)
            .await
            .unwrap();
    }
    let mut ignored = Vec::new();
    for n in 0..32 {
        let body=json!({"action":"opened","installation":{"id":99},"repository":{"id":2000+n,"owner":{"id":123}},"issue":{"id":9000+n,"number":n+1}}).to_string();
        ignored.push(
            f.webhook(
                &url,
                "fixture-webhook-secret",
                &format!("burst-{n}"),
                "issues",
                &body,
            )
            .send(),
        );
    }
    let filtered=json!({"action":"observed","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":77,"number":1}}).to_string();
    ignored.push(
        f.webhook(
            &url,
            "fixture-webhook-secret",
            "approved-filtered",
            "issues",
            &filtered,
        )
        .send(),
    );
    let suspend =
        json!({"action":"suspend","installation":{"id":99,"app_id":42,"account":{"id":123}}})
            .to_string();
    let lifecycle = f
        .webhook(
            &url,
            "fixture-webhook-secret",
            "burst-suspend",
            "installation",
            &suspend,
        )
        .send();
    tokio::pin!(lifecycle);
    let started = tokio::time::Instant::now();
    let results = tokio::select! {
        result=tokio::time::timeout(std::time::Duration::from_secs(1),futures_util::future::join_all(ignored))=>result,
        result=&mut lifecycle=>panic!("lifecycle should await the held external write: {:?}",result.map(|r|r.status())),
    };
    session.abort_transaction().await.unwrap();
    for response in
        results.expect("inert receipts must complete while authority writes are contended")
    {
        let response = response.unwrap();
        assert_eq!(response.status(), 200);
        let receipt: serde_json::Value = response.json().await.unwrap();
        assert_eq!(receipt["status"], "ignored");
        assert_eq!(receipt["task_ids"], json!([]));
    }
    let response = lifecycle.await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    let after=f.webhook(&url,"fixture-webhook-secret","approved-after-suspend","issues",&json!({"action":"opened","installation":{"id":99},"repository":{"id":1001,"owner":{"id":123}},"issue":{"id":78,"number":2}}).to_string()).send().await.unwrap();
    assert_eq!(after.status(), 200);
    assert_eq!(
        after.json::<serde_json::Value>().await.unwrap()["reason"],
        "installation_suspended"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn binding_cas_race_reports_the_current_management_version() {
    use mongodb::bson::doc;
    let mut f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let application = format!("binding-cas-{}", uuid::Uuid::new_v4());
    f.database.uri.push_str(&format!("&appName={application}"));
    f.base = f.replica().await;
    f.database.admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":{"failCommands":["update"],"appName":application,"blockConnection":true,"blockTimeMS":700}}).await.unwrap();
    let approval = verification(&f, &binding, "token-alice", "raced-approval").send();
    tokio::pin!(approval);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut approval)
            .await
            .is_err()
    );
    let body=json!({"action":"removed","installation":{"id":99,"app_id":42,"account":{"id":123}},"repositories_added":[],"repositories_removed":[{"id":9999}]}).to_string();
    let lifecycle = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            "racing-removal",
            "installation_repositories",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(lifecycle.status(), 200);
    let response = approval.await.unwrap();
    assert_eq!(response.status(), 409);
    let problem: serde_json::Value = response.json().await.unwrap();
    assert!(
        problem["detail"].as_str().unwrap().contains("version is 2"),
        "{problem}"
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn management_creation_and_approval_retry_transient_authority_contention() {
    use mongodb::bson::doc;
    let f = Fixture::new(true).await;
    let (app, binding) = pending(&f).await;
    let mut session = f.database.admin.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    f.database
        .admin
        .database(&f.database.name)
        .collection::<mongodb::bson::Document>("eg_automation_records")
        .update_one(
            doc! {"_id":app["app_ref"].as_str().unwrap()},
            doc! {"$inc":{"version":1_i64}},
        )
        .session(&mut session)
        .await
        .unwrap();
    let created=f.client.post(format!("{}/v1/github/installation-bindings",f.base)).bearer_auth("token-alice").header("Idempotency-Key","contended-create")
        .json(&json!({"app_ref":app["app_ref"],"installation_id":100,"account_id":123,"repository_ids":[1001]})).send();
    let approved = verification(&f, &binding, "token-alice", "contended-approve").send();
    let both = async { tokio::join!(created, approved) };
    tokio::pin!(both);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut both)
            .await
            .is_err()
    );
    session.abort_transaction().await.unwrap();
    let (created, approved) = both.await;
    assert_eq!(created.unwrap().status(), 201);
    assert_eq!(approved.unwrap().status(), 200);
}
