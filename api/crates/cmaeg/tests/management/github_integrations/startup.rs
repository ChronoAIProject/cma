use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn disabled_startup_does_not_prepare_storage_even_with_migration_authority() {
    use mongodb::bson::doc;
    let mut fixture = Fixture::new(false).await;
    fixture.allow_startup_migration().await;
    let disabled = fixture.replica_with_flag(Some(false)).await;
    assert_eq!(
        fixture
            .read(&disabled, "/v1/channels", "token-alice")
            .await
            .status(),
        200
    );
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .run_command(cmaeg::github::runtime_role(&fixture.database.name))
        .await
        .unwrap();
    database.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_automation_runtime","db":&fixture.database.name}]}).await.unwrap();
    database.run_command(doc! {"revokeRolesFromUser":"runtime","roles":[{"role":"cma_eg_automation_migrator","db":&fixture.database.name}]}).await.unwrap();
    let enabled_without_initialization_authority = fixture.replica().await;
    let capabilities = fixture
        .read(
            &enabled_without_initialization_authority,
            "/v1/github/capabilities",
            "token-alice",
        )
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["store_ready"], false);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn unknown_future_schema_is_not_activated_or_downgraded_by_startup() {
    use mongodb::bson::doc;
    let mut fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "future-reader", None).await;
    fixture.allow_startup_migration().await;
    let markers = fixture
        .database
        .admin
        .database(&fixture.database.name)
        .collection::<mongodb::bson::Document>("eg_schema_migrations");
    markers
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":99}},
        )
        .bypass_document_validation(true)
        .await
        .unwrap();
    for _attempt in 0..2 {
        let incompatible = fixture.replica().await;
        let capabilities = fixture
            .read(&incompatible, "/v1/github/capabilities", "token-alice")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(capabilities["store_ready"], false);
        assert_eq!(
            fixture
                .read(&incompatible, "/v1/channels", "token-alice")
                .await
                .status(),
            200
        );
    }
    markers
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":2}},
        )
        .await
        .unwrap();
    let compatible = fixture.replica().await;
    assert_eq!(
        fixture
            .read(
                &compatible,
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
async fn incomplete_upgrade_stays_unavailable_and_resumes_after_the_external_fault_is_fixed() {
    use mongodb::{bson::doc, options::IndexOptions, IndexModel};
    let mut fixture = Fixture::new(true).await;
    let app = fixture
        .register("token-alice", "before-interruption", None)
        .await;
    fixture.allow_startup_migration().await;
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":1}},
        )
        .await
        .unwrap();
    let records = database.collection::<mongodb::bson::Document>("eg_automation_records");
    records.drop_index("owner_1_kind_1__id_1").await.unwrap();
    records
        .create_index(
            IndexModel::builder()
                .keys(doc! {"owner":1,"kind":1,"_id":1})
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await
        .unwrap();
    let interrupted = fixture.replica().await;
    let capabilities = fixture
        .read(&interrupted, "/v1/github/capabilities", "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["store_ready"], false);
    assert_eq!(
        fixture
            .registration(
                &interrupted,
                "token-alice",
                "during-interruption",
                &json!({"app_id":43,"display_name":"Unavailable"})
            )
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert_eq!(
        fixture
            .read(&interrupted, "/v1/channels", "token-alice")
            .await
            .status(),
        200
    );
    records.drop_index("owner_1_kind_1__id_1").await.unwrap();
    let recovered = fixture.replica().await;
    assert_eq!(
        fixture
            .read(
                &recovered,
                &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        app
    );
    assert_eq!(
        fixture
            .registration(
                &recovered,
                "token-alice",
                "after-interruption",
                &json!({"app_id":43,"display_name":"Recovered"})
            )
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn concurrent_startups_converge_on_one_usable_integration_store() {
    let mut fixture = Fixture::new(false).await;
    fixture.allow_startup_migration().await;
    let mut startups = Vec::new();
    for number in 0..2 {
        let address = free_address();
        let mut command =
            fixture
                .files
                .command(&address, &fixture.cma, &fixture.nyxid, &fixture.provider);
        fixture.database.configure(&mut command);
        command
            .env("CMAEG_GITHUB_ENABLED", "true")
            .env("CMAEG_MONGODB_AUTHORITY_MODE", "shared-schema")
            .env("CMAEG_PUBLIC_API_URL", &fixture.base)
            .env("CMAEG_CMA_BASE_URL", fixture.provider.base_url());
        let log = fixture.files.dir.join(format!("parallel-{number}.log"));
        startups.push(async move {
            (
                format!("http://{address}"),
                spawn_ready(command, &address, &log).await,
            )
        });
    }
    let mut bases = Vec::new();
    for (base, child) in futures_util::future::join_all(startups).await {
        fixture.children.push(child);
        bases.push(base);
    }
    for number in 0..2 {
        let log = std::fs::read_to_string(fixture.files.dir.join(format!("parallel-{number}.log")))
            .unwrap();
        for line in log.lines().filter(|line| line.contains("GitHub")) {
            eprintln!("{line}");
        }
    }
    let body = json!({"app_id":42,"display_name":"Concurrent startup"});
    let created = fixture
        .registration(&bases[0], "token-alice", "parallel-app", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let replayed = fixture
        .registration(&bases[1], "token-alice", "parallel-app", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(replayed.status(), 200);
    assert_eq!(
        created.json::<serde_json::Value>().await.unwrap(),
        replayed.json::<serde_json::Value>().await.unwrap()
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn enabled_startup_initializes_storage_and_restart_retains_app_and_receipt() {
    let mut fixture = Fixture::new(false).await;
    fixture.allow_startup_migration().await;
    fixture.base.clear();
    fixture.base = fixture.replica().await;
    let app = fixture
        .register(
            "token-alice",
            "startup-app",
            Some("synthetic-startup-webhook-secret"),
        )
        .await;
    let webhook_url = app["webhook_url"].as_str().unwrap();
    let ping = fixture
        .webhook(
            webhook_url,
            "synthetic-startup-webhook-secret",
            "startup-ping",
            "ping",
            r#"{"zen":"Startup proof","hook_id":42,"hook":{}}"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(ping.status(), 200);
    let receipt = ping.json::<serde_json::Value>().await.unwrap();
    for child in &mut fixture.children {
        child.kill().await.unwrap();
    }
    let restarted = fixture.replica().await;
    assert_eq!(
        fixture
            .read(
                &restarted,
                &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        app
    );
    assert_eq!(
        fixture
            .read(
                &restarted,
                &format!(
                    "/v1/github/deliveries/{}",
                    receipt["receipt_id"].as_str().unwrap()
                ),
                "token-alice"
            )
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap(),
        receipt
    );
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn disabled_or_absent_flag_refuses_admission_and_preserves_integrations() {
    let mut fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "retained-app", None).await;
    for enabled in [None, Some(false)] {
        let disabled = fixture.replica_with_flag(enabled).await;
        let capabilities = fixture
            .read(&disabled, "/v1/github/capabilities", "token-alice")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        assert_eq!(capabilities["store_ready"], false);
        assert_eq!(
            fixture
                .registration(
                    &disabled,
                    "token-alice",
                    "disabled-app",
                    &json!({"app_id":43,"display_name":"Disabled"})
                )
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
        assert_eq!(
            fixture
                .read(&disabled, "/v1/channels", "token-alice")
                .await
                .status(),
            200
        );
        let webhook = format!(
            "{disabled}/v1/github/apps/{}/webhooks",
            app["app_ref"].as_str().unwrap()
        );
        assert_eq!(
            fixture
                .webhook(
                    &webhook,
                    "synthetic-disabled-webhook-secret",
                    "disabled-ping",
                    "ping",
                    r#"{"hook_id":42,"hook":{}}"#
                )
                .send()
                .await
                .unwrap()
                .status(),
            503
        );
    }
    let enabled = fixture.replica().await;
    let restored = fixture
        .read(
            &enabled,
            &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
            "token-alice",
        )
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(restored, app);
}
