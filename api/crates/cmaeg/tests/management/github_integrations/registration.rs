use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn app_updates_require_the_owner_and_a_current_version() {
    let fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "editable-app", None).await;
    let app_ref = app["app_ref"].as_str().unwrap();
    let url = format!("{}/v1/github/apps/{app_ref}", fixture.base);
    let update = json!({"expected_version":1,"display_name":"Renamed integration"});
    let forbidden = fixture
        .client
        .patch(&url)
        .bearer_auth("token-bob")
        .header("Idempotency-Key", "rename-app")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 404);
    let changed = fixture
        .client
        .patch(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "rename-app")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), 200);
    let changed: serde_json::Value = changed.json().await.unwrap();
    assert_eq!(changed["version"], 2);
    assert_eq!(changed["display_name"], "Renamed integration");
    assert_eq!(changed["app_id"], 42);
    let stale = fixture
        .client
        .patch(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "another-update")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    let replay = fixture
        .client
        .patch(&url)
        .bearer_auth("token-alice")
        .header("Idempotency-Key", "rename-app")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.json::<serde_json::Value>().await.unwrap(), changed);
    assert_eq!(
        fixture
            .read(
                &fixture.base,
                &format!("/v1/github/apps/{app_ref}"),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        changed
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn old_schema_waits_for_startup_authority_then_upgrades_without_a_migration_command() {
    let mut fixture = Fixture::new(true).await;
    let app = fixture
        .register("token-alice", "before-upgrade", None)
        .await;
    fixture.children[0].kill().await.unwrap();
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            mongodb::bson::doc! {"_id":"github_integrations"},
            mongodb::bson::doc! {"$set":{"version":1}},
        )
        .await
        .unwrap();
    let outdated = fixture.replica_with_flag(Some(true)).await;
    let capabilities = fixture
        .read(&outdated, "/v1/github/capabilities", "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["store_ready"], false);
    assert_eq!(
        fixture
            .registration(
                &outdated,
                "token-alice",
                "old-schema",
                &json!({"app_id":42,"display_name":"Not ready"})
            )
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert_eq!(
        fixture
            .read(&outdated, "/v1/channels", "token-alice")
            .await
            .status(),
        200
    );
    fixture.allow_startup_migration().await;
    let upgraded = fixture.replica().await;
    let capabilities = fixture
        .read(&upgraded, "/v1/github/capabilities", "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["store_ready"], true);
    assert_eq!(capabilities["native_app_webhook"], true);
    assert_eq!(capabilities["app_identity_verification"], true);
    assert_eq!(capabilities["ownership_verification"], false);
    assert_eq!(capabilities["automatic_reconciliation"], false);
    assert_eq!(
        fixture
            .read(
                &upgraded,
                &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        app
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn registers_and_reads_a_redacted_github_app_without_startup_configuration() {
    let fixture = Fixture::new(true).await;
    let base = &fixture.base;
    let secret = "synthetic-github-webhook-custody-sentinel";
    let response = fixture
        .registration(
            base,
            "token-alice",
            "register-app-first-slice",
            &json!({"app_id":42,"display_name":"Issue automation","webhook_secret":secret}),
        )
        .header("Host", "untrusted-webhook-origin.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.unwrap();
    assert!(!body.contains(secret));
    let created: serde_json::Value = serde_json::from_str(&body).unwrap();
    let app_ref = created["app_ref"].as_str().unwrap();
    assert!(!app_ref.is_empty());
    assert_eq!(created["version"], 1);
    assert_eq!(created["status"], "pending_verification");
    assert_eq!(created["webhook_credential"]["present"], true);
    assert_eq!(created["verification_credential"]["present"], false);
    assert_eq!(
        created["webhook_url"],
        format!("{base}/v1/github/apps/{app_ref}/webhooks")
    );
    let read = fixture
        .read(base, &format!("/v1/github/apps/{app_ref}"), "token-alice")
        .await;
    assert_eq!(read.status(), 200);
    assert_eq!(read.json::<serde_json::Value>().await.unwrap(), created);
    assert!(
        !std::fs::read_to_string(fixture.files.dir.join("github-0.log"))
            .unwrap()
            .contains(secret)
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn app_inventory_is_owner_scoped_and_visible_to_an_already_running_replica() {
    let mut fixture = Fixture::new(true).await;
    let second = fixture.replica().await;
    let body = json!({"app_id":42,"display_name":"Tenant-owned draft"});
    let response = fixture
        .registration(&fixture.base, "token-alice", "same-key", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    let list = fixture
        .read(&second, "/v1/github/apps", "token-alice")
        .await;
    assert_eq!(list.status(), 200);
    assert_eq!(
        list.json::<serde_json::Value>().await.unwrap()["items"],
        json!([created])
    );
    let other = fixture.read(&second, "/v1/github/apps", "token-bob").await;
    assert_eq!(other.status(), 200);
    assert_eq!(
        other.json::<serde_json::Value>().await.unwrap()["items"],
        json!([])
    );
    let invisible = fixture
        .read(
            &second,
            &format!("/v1/github/apps/{}", created["app_ref"].as_str().unwrap()),
            "token-bob",
        )
        .await;
    assert_eq!(invisible.status(), 404);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn lost_registration_response_recovers_after_restart_and_changed_secret_conflicts() {
    let mut fixture = Fixture::new(true).await;
    let secret = "synthetic-github-registration-recovery-sentinel";
    let mut body =
        json!({"app_id":42,"display_name":"Recoverable registration","webhook_secret":secret});
    let original = fixture
        .registration(&fixture.base, "token-alice", "lost-result", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(original.status(), 201);
    drop(original);
    fixture.children[0].kill().await.unwrap();
    let restarted = fixture.replica().await;
    let recovered = fixture
        .registration(&restarted, "token-alice", "lost-result", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(recovered.status(), 200);
    let integration: serde_json::Value = recovered.json().await.unwrap();
    let inventory = fixture
        .read(&restarted, "/v1/github/apps", "token-alice")
        .await;
    assert_eq!(
        inventory.json::<serde_json::Value>().await.unwrap()["items"],
        json!([integration])
    );
    body["webhook_secret"] = json!("synthetic-different-webhook-sentinel");
    let conflict = fixture
        .registration(&restarted, "token-alice", "lost-result", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    let diagnostic = conflict.text().await.unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&diagnostic).unwrap()["code"],
        "conflict"
    );
    assert!(!diagnostic.contains(secret));
    assert!(!diagnostic.contains("synthetic-different-webhook-sentinel"));
    let unchanged = fixture
        .read(
            &restarted,
            &format!(
                "/v1/github/apps/{}",
                integration["app_ref"].as_str().unwrap()
            ),
            "token-alice",
        )
        .await;
    assert_eq!(
        unchanged.json::<serde_json::Value>().await.unwrap(),
        integration
    );
    let other_owner = fixture
        .registration(&restarted, "token-bob", "lost-result", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(other_owner.status(), 201);
    assert_ne!(
        other_owner.json::<serde_json::Value>().await.unwrap()["app_ref"],
        integration["app_ref"]
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn missing_startup_authority_refuses_onboarding_without_disabling_the_gateway() {
    let fixture = Fixture::new(false).await;
    let capabilities = fixture
        .read(&fixture.base, "/v1/github/capabilities", "token-alice")
        .await;
    assert_eq!(capabilities.status(), 200);
    let capabilities: serde_json::Value = capabilities.json().await.unwrap();
    assert_eq!(capabilities["store_ready"], false);
    assert_eq!(capabilities["dynamic_app_registration"], true);
    assert_eq!(capabilities["automatic_reconciliation"], false);
    let registration = fixture
        .registration(
            &fixture.base,
            "token-alice",
            "not-migrated",
            &json!({"app_id":42,"display_name":"Unavailable registration"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(registration.status(), 503);
    let channels = fixture
        .read(&fixture.base, "/v1/channels", "token-alice")
        .await;
    assert_eq!(channels.status(), 200);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn concurrent_registration_retries_converge_on_one_app() {
    let mut fixture = Fixture::new(true).await;
    let second = fixture.replica().await;
    let body = json!({"app_id":42,"display_name":"Concurrent registration","webhook_secret":"synthetic-concurrent-webhook-sentinel"});
    let responses = futures_util::future::join_all((0..8).map(|index| {
        let base = if index % 2 == 0 {
            &fixture.base
        } else {
            &second
        };
        fixture
            .registration(base, "token-alice", "one-logical-registration", &body)
            .send()
    }))
    .await;
    let mut identities = std::collections::BTreeSet::new();
    for response in responses {
        let mut response = response.unwrap();
        if response.status() == 503 {
            response = fixture
                .registration(&second, "token-alice", "one-logical-registration", &body)
                .send()
                .await
                .unwrap();
        }
        assert!(matches!(response.status().as_u16(), 200 | 201));
        let integration: serde_json::Value = response.json().await.unwrap();
        identities.insert(integration["app_ref"].as_str().unwrap().to_owned());
    }
    assert_eq!(identities.len(), 1);
    let listed = fixture
        .read(&second, "/v1/github/apps", "token-alice")
        .await;
    assert_eq!(
        listed.json::<serde_json::Value>().await.unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn registration_requires_creator_identity_and_refuses_body_selected_ownership() {
    let fixture = Fixture::new(true).await;
    let mut body = json!({"app_id":42,"display_name":"Protected registration","webhook_secret":"synthetic-protected-webhook-sentinel"});
    let unauthenticated = fixture
        .client
        .post(format!("{}/v1/github/apps", fixture.base))
        .header("Idempotency-Key", "unauthenticated")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), 401);
    body["owner"] = json!("bob");
    let forged = fixture
        .registration(&fixture.base, "token-alice", "forged-owner", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 400);
    assert!(!forged
        .text()
        .await
        .unwrap()
        .contains("synthetic-protected-webhook-sentinel"));
    let inventory = fixture
        .read(&fixture.base, "/v1/github/apps", "token-alice")
        .await;
    assert_eq!(
        inventory.json::<serde_json::Value>().await.unwrap()["items"],
        json!([])
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn app_inventory_pages_without_repeating_apps_or_exposing_credentials() {
    let fixture = Fixture::new(true).await;
    let mut expected = std::collections::BTreeSet::new();
    for index in 0..3 {
        let response = fixture
            .registration(
                &fixture.base,
                "token-alice",
                &format!("page-{index}"),
                &json!({"app_id":42 + index,"display_name":"Paged draft"}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let integration: serde_json::Value = response.json().await.unwrap();
        expected.insert(integration["app_ref"].as_str().unwrap().to_owned());
    }
    let first = fixture
        .read(&fixture.base, "/v1/github/apps?limit=2", "token-alice")
        .await;
    let first: serde_json::Value = first.json().await.unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 2);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = fixture
        .read(
            &fixture.base,
            &format!("/v1/github/apps?limit=2&after={cursor}"),
            "token-alice",
        )
        .await;
    let second: serde_json::Value = second.json().await.unwrap();
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second.get("next_cursor").is_none());
    let observed: std::collections::BTreeSet<String> = first["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second["items"].as_array().unwrap())
        .map(|app| app["app_ref"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(observed, expected);
    let invalid = fixture
        .read(&fixture.base, "/v1/github/apps?limit=101", "token-alice")
        .await;
    assert_eq!(invalid.status(), 400);
}
