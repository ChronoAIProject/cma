use cmaeg::records::{
    import::import_with,
    mongo::{self, MongoRecords},
    ImportGate, RecordError,
};
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture with failpoints"]
async fn migration_cli_reports_uncertain_abort_on_stderr_without_rust_log() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("migration_cli_{}", uuid::Uuid::new_v4().simple());
    let database = admin.database(&name);
    let mut cli_uri = url::Url::parse(&uri).unwrap();
    let app = format!("migration_cli_{}", uuid::Uuid::new_v4().simple());
    cli_uri.query_pairs_mut().append_pair("appName", &app);

    for refused_abort in [false, true] {
        let commands = if refused_abort {
            vec!["findAndModify", "abortTransaction"]
        } else {
            vec!["findAndModify"]
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand",
                "mode": {"times": commands.len() as i32},
                "data": {"appName": &app, "failCommands": commands, "errorCode": 2_i32},
            })
            .await
            .unwrap();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            tokio::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
                .arg("--migrate")
                .env_remove("RUST_LOG")
                .env("CMAEG_MONGODB_DATABASE", &name)
                .env("CMAEG_MONGODB_URI", cli_uri.as_str())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("migration database operation unavailable"),
            "{stderr}"
        );
        assert_eq!(
            stderr.contains("migration abort command failed"),
            refused_abort,
            "{stderr}"
        );
    }
    database.drop().await.unwrap();
}

#[path = "support/activity_history_backfill.rs"]
mod activity_history_backfill;
#[path = "support/github_import.rs"]
mod github_import;
#[path = "support/record_domains.rs"]
mod record_domains;
#[path = "support/shared_schema.rs"]
mod shared_schema;
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn gateway_records_are_exclusive_owner_immutable_and_revision_fenced() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_eg_contract_{}", uuid::Uuid::new_v4().simple());
    let result = exercise(&uri, &admin, &name).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn disabled_github_import_refuses_new_github_channels_and_retains_stored_ones() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_eg_github_import_{}", uuid::Uuid::new_v4().simple());
    let result = github_import::verify(&admin.database(&name)).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}
async fn exercise(uri: &str, admin: &Client, name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let db = admin.database(name);
    db.run_command(mongo::role(name, false)).await?;
    db.run_command(mongo::role(name, true)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    for (user, role) in [
        ("gateway-runtime", "cma_eg_runtime"),
        ("gateway-migrator", "cma_eg_migrator"),
    ] {
        db.run_command(doc! {"createUser":user,"pwd":&password,"roles":[{"role":role,"db":name}]})
            .await?;
    }
    let connect = |user: &'static str| {
        let password = password.clone();
        async move {
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username(user.to_owned())
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            Client::with_options(options)
        }
    };
    let runtime = connect("gateway-runtime").await?;
    let migrator = connect("gateway-migrator").await?;
    assert!(MongoRecords::open(db.clone()).await.is_err());
    assert!(MongoRecords::open(runtime.database(name)).await.is_err());
    mongo::migrate(&migrator.database(name)).await?;
    mongo::migrate(&migrator.database(name)).await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    for foreign in [
        "agents",
        "sandboxes",
        "auth_subjects",
        "gateway_channel_records",
        "im_images",
        "im_builds",
        "im_auth_sessions",
    ] {
        db.collection::<Document>(foreign)
            .insert_one(doc! {"_id":"private"})
            .await?;
        for client in [&runtime, &migrator] {
            let collection = client.database(name).collection::<Document>(foreign);
            assert!(denied(collection.find_one(doc! {}).await.unwrap_err()));
            assert!(denied(
                collection
                    .insert_one(doc! {"_id":"attempt"})
                    .await
                    .unwrap_err()
            ));
        }
    }
    let imported_id = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir().join(format!("cma-record-export-{}", uuid::Uuid::new_v4()));
    let export = serde_json::json!({"record_id":imported_id,"owner":"imported-owner","version":"7","blob":"b3BhcXVl"}).to_string();
    std::fs::write(&path, format!("{export}\n"))?;
    // Opaque record mechanics only; the GitHub write gate has its own contract.
    let gate = ImportGate::new(true, None);
    let imported = import_with(&migrator.database(name), &path, &gate).await?;
    assert_eq!(imported, 1);
    assert_eq!(
        import_with(&migrator.database(name), &path, &gate).await?,
        1
    );
    assert_eq!(records.get(&imported_id).await?.version, 7);
    records
        .put(&imported_id, "imported-owner", b"changed", Some(7))
        .await?;
    assert!(import_with(&migrator.database(name), &path, &gate)
        .await
        .is_err());
    assert_eq!(records.get(&imported_id).await?.blob, b"changed");
    std::fs::remove_file(path)?;
    assert_eq!(records.put("record", "alice", b"opaque-v1", None).await?, 1);
    assert!(matches!(
        records.put("record", "bob", b"forged", Some(1)).await,
        Err(RecordError::Conflict { .. })
    ));
    let attempts = futures_util::future::join_all(
        (0..8).map(|_| records.put("record", "alice", b"opaque-v2", Some(1))),
    )
    .await;
    assert_eq!(attempts.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(records.list(Some("bob"), None, 50).await?.is_empty());
    assert_eq!(records.list(Some("alice"), None, 50).await?.len(), 1);
    let restarted = MongoRecords::open(runtime.database(name)).await?;
    assert_eq!(restarted.get("record").await?.blob, b"opaque-v2");
    assert!(records.delete("record", 1).await.is_err());
    assert!(records.delete("record", 2).await?);
    record_domains::verify(&db, restarted).await?;
    Ok(())
}
fn denied(e: mongodb::error::Error) -> bool {
    matches!(e.kind.as_ref(),mongodb::error::ErrorKind::Command(c) if c.code==13)
}
