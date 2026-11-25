use super::*;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn missing_base_migration_authority_keeps_github_unavailable_but_http_healthy() {
    use mongodb::bson::doc;
    let mut fixture = Fixture::new(true).await;
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .delete_one(doc! {"_id":"github_channel_guard"})
        .await
        .unwrap();
    for child in &mut fixture.children {
        child.kill().await.unwrap();
    }
    let base = fixture.replica_with_flag(Some(true)).await;
    let channels = fixture.read(&base, "/v1/channels", "token-alice").await;
    assert_eq!(channels.status(), 200);
    let capabilities = fixture
        .read(&base, "/v1/github/capabilities", "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(capabilities["store_ready"], false);
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn old_channel_validator_is_upgraded_before_github_admission_without_manual_migrate() {
    use futures_util::TryStreamExt;
    use mongodb::bson::doc;
    let mut fixture = Fixture::new(true).await;
    fixture.allow_startup_migration().await;
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .delete_one(doc! {"_id":"github_channel_guard"})
        .await
        .unwrap();
    let old_validator = doc! {
        "$jsonSchema": {
            "bsonType":"object",
            "additionalProperties":false,
            "required":["_id","owner","blob","version","schema_version"],
            "properties": {
                "_id":{"bsonType":"string"},
                "owner":{"bsonType":"string","minLength":1,"maxLength":512},
                "blob":{"bsonType":"binData"},
                "version":{"bsonType":"long","minimum":1},
                "schema_version":{"enum":[1]}
            }
        }
    };
    database
        .run_command(doc! {
            "collMod":"eg_channel_records",
            "validator":old_validator,
            "validationLevel":"strict",
            "validationAction":"error"
        })
        .await
        .unwrap();
    for child in &mut fixture.children {
        child.kill().await.unwrap();
    }
    fixture.base = fixture.replica().await;
    let marker = database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .find_one(doc! {"_id":"github_channel_guard","version":1})
        .await
        .unwrap();
    assert!(
        marker.is_some(),
        "startup must install the additive guard marker"
    );
    let collections = database
        .list_collections()
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    let validator = collections
        .into_iter()
        .find(|info| info.name == "eg_channel_records")
        .and_then(|info| info.options.validator);
    assert!(validator
        .as_ref()
        .is_some_and(|validator| validator.to_string().contains("guard_revision")));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn blocked_github_initialization_keeps_http_available_and_recovers_without_restart() {
    use mongodb::bson::doc;
    use std::time::Duration;
    let mut fixture = Fixture::new(true).await;
    fixture.allow_startup_migration().await;
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":1}},
        )
        .write_concern(
            mongodb::options::WriteConcern::builder()
                .w(mongodb::options::Acknowledgment::Majority)
                .build(),
        )
        .await
        .unwrap();
    let mut session = fixture.database.admin.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":2}},
        )
        .session(&mut session)
        .await
        .unwrap();
    let started = tokio::time::timeout(
        Duration::from_secs(3),
        fixture.replica_with_flag(Some(true)),
    )
    .await;
    let observed = if let Ok(base) = &started {
        let channels = fixture
            .read(base, "/v1/channels", "token-alice")
            .await
            .status();
        let capabilities = fixture
            .read(base, "/v1/github/capabilities", "token-alice")
            .await
            .json::<serde_json::Value>()
            .await
            .unwrap();
        Some((channels, capabilities))
    } else {
        None
    };
    session.abort_transaction().await.unwrap();
    let base = started.expect("optional GitHub initialization must not delay HTTP startup");
    let (channels, capabilities) = observed.unwrap();
    assert_eq!(channels, 200);
    assert_eq!(capabilities["store_ready"], false);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let capabilities = fixture
                .read(&base, "/v1/github/capabilities", "token-alice")
                .await
                .json::<serde_json::Value>()
                .await
                .unwrap();
            if capabilities["store_ready"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("GitHub becomes available after the database resumes");
    let response = fixture
        .registration(
            &base,
            "token-alice",
            "after-delayed-init",
            &json!({"app_id":42,"display_name":"Recovered without restart"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}

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
    let enabled_without_initialization_authority = fixture.replica_with_flag(Some(true)).await;
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
        let incompatible = fixture.replica_with_flag(Some(true)).await;
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
    let interrupted = fixture.replica_with_flag(Some(true)).await;
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
    let database = fixture.database.admin.database(&fixture.database.name);
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .delete_one(mongodb::bson::doc! {"_id":"github_channel_guard"})
        .await
        .unwrap();
    let ledger = database.collection::<mongodb::bson::Document>(cmaeg::migrations::LEDGER);
    let baseline_fence = ledger
        .find_one(mongodb::bson::doc! {"_id":"runner"})
        .await
        .unwrap()
        .unwrap()
        .get_i64("fence")
        .unwrap();
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
        fixture.wait_for_github(&base).await;
        bases.push(base);
    }
    for number in 0..2 {
        let log = std::fs::read_to_string(fixture.files.dir.join(format!("parallel-{number}.log")))
            .unwrap();
        for line in log.lines().filter(|line| line.contains("GitHub")) {
            eprintln!("{line}");
        }
    }
    for migration in ["gateway-base-v2", "gateway-automation-v6"] {
        assert_eq!(ledger.count_documents(mongodb::bson::doc!{"kind":"attempt","migration":migration,"outcome":"completed","fence":{"$gt":baseline_fence},"effects":{"$gt":0}}).await.unwrap(),1);
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
async fn explicitly_disabled_flag_refuses_admission_and_preserves_integrations() {
    let mut fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "retained-app", None).await;
    {
        let disabled = fixture.replica_with_flag(Some(false)).await;
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

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn legacy_receipt_retention_is_stamped_once_across_repeatable_upgrade() {
    use mongodb::bson::{doc, spec::BinarySubtype, Binary};
    let mut f = Fixture::new(true).await;
    let app = f
        .register(
            "token-alice",
            "legacy-receipt",
            Some("fixture-webhook-secret"),
        )
        .await;
    let body = json!({"zen":"legacy","hook_id":7,"hook":{"type":"App"}}).to_string();
    let response = f
        .webhook(
            &format!(
                "{}/v1/github/apps/{}/webhooks",
                f.base,
                app["app_ref"].as_str().unwrap()
            ),
            "fixture-webhook-secret",
            "legacy-ping",
            "ping",
            &body,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut receipt: serde_json::Value = response.json().await.unwrap();
    receipt.as_object_mut().unwrap().remove("retained_until");
    let id = receipt["receipt_id"].as_str().unwrap().to_owned();
    let old=serde_json::to_vec(&json!({"id":id,"owner":"alice","kind":"delivery","value":{"receipt":receipt,"raw_body_records":[]}})).unwrap();
    let blob = cmaeg_core::seal::seal_blob(&key_ring(1), &old).unwrap();
    for child in &mut f.children {
        child.kill().await.unwrap();
    }
    let database = f.database.admin.database(&f.database.name);
    database.collection::<mongodb::bson::Document>("eg_automation_records").update_one(doc! {"_id":&id},doc! {"$set":{"blob":Binary{subtype:BinarySubtype::Generic,bytes:blob}},"$unset":{"expires_at":""}}).bypass_document_validation(true).await.unwrap();
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":3}},
        )
        .await
        .unwrap();
    f.allow_startup_migration().await;
    f.base = f.replica().await;
    let path = format!("/v1/github/deliveries/{id}");
    let migrated = f
        .read(&f.base, &path, "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert!(
        migrated["retained_until"].as_i64().unwrap()
            >= migrated["received_at"].as_i64().unwrap() + 7 * 86400
    );
    database
        .collection::<mongodb::bson::Document>("eg_schema_migrations")
        .update_one(
            doc! {"_id":"github_integrations"},
            doc! {"$set":{"version":3}},
        )
        .await
        .unwrap();
    let again = f.replica().await;
    let reread = f
        .read(&again, &path, "token-alice")
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(reread, migrated);
}

/// `CMAEG_GITHUB_ENABLED` has graduated: an absent flag behaves as `true`.
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn absent_flag_admits_like_explicitly_enabled() {
    let mut fixture = Fixture::new(true).await;
    let app = fixture.register("token-alice", "retained-app", None).await;
    let absent = fixture.replica_with_flag(None).await;
    fixture.wait_for_github(&absent).await;
    assert_eq!(
        fixture
            .registration(
                &absent,
                "token-alice",
                "absent-flag-app",
                &json!({"app_id":43,"display_name":"Absent flag"})
            )
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    let restored = fixture
        .read(
            &absent,
            &format!("/v1/github/apps/{}", app["app_ref"].as_str().unwrap()),
            "token-alice",
        )
        .await
        .json::<serde_json::Value>()
        .await
        .unwrap();
    assert_eq!(restored, app);
}
