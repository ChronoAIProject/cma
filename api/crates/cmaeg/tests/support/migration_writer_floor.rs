//! Verify refusal at the real operator command before any floor schema change.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};
use std::{process::Output, time::Duration};
use tokio::process::Command;

const LEDGER: &str = cmaeg::migrations::LEDGER;
const MARKERS: &str = "eg_schema_migrations";
const FENCES: &str = "eg_channel_create_fences";

async fn cli(db: &Database, uri: &url::Url, args: &[&str]) -> Output {
    tokio::time::timeout(
        Duration::from_secs(20),
        Command::new(env!("CARGO_BIN_EXE_cmaeg"))
            .args(args)
            .env("CMAEG_MONGODB_DATABASE", db.name())
            .env("CMAEG_MONGODB_URI", uri.as_str())
            .env("CMAEG_NYXID_BASE_URL", "https://issuer.example")
            .env_remove("RUST_LOG")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap()
}

async fn activate(db: &Database, uri: &url::Url) {
    db.run_command(cma_service_migrations::operator_role(db.name(), LEDGER))
        .await
        .unwrap();
    let status = db.run_command(doc! {"connectionStatus":1}).await.unwrap();
    let user = status
        .get_document("authInfo")
        .unwrap()
        .get_array("authenticatedUsers")
        .unwrap()[0]
        .as_document()
        .unwrap();
    db.client()
        .database(user.get_str("db").unwrap())
        .run_command(doc! {
            "grantRolesToUser":user.get_str("user").unwrap(),
            "roles":[{"role":format!("{LEDGER}_operator"),"db":db.name()}]
        })
        .await
        .unwrap();
    let activated = cli(db, uri, &["--migration-activate", "synthetic writer drain"]).await;
    assert!(
        activated.status.success(),
        "{}",
        String::from_utf8_lossy(&activated.stderr)
    );
}

async fn snapshot(db: &Database) -> (Vec<Document>, Vec<Document>, Vec<Document>) {
    let mut schemas: Vec<_> = db.list_collections().await.unwrap()
        .try_collect::<Vec<_>>().await.unwrap().into_iter()
        .filter(|collection| collection.name != LEDGER)
        .map(|collection| {
            doc! {"name":collection.name, "options":mongodb::bson::serialize_to_document(&collection.options).unwrap()}
        }).collect();
    schemas.sort_by(|left, right| {
        left.get_str("name")
            .unwrap()
            .cmp(right.get_str("name").unwrap())
    });
    let markers = db
        .collection::<Document>(MARKERS)
        .find(doc! {})
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let floors = db
        .collection::<Document>(FENCES)
        .find(doc! {"_id":"writer_floor"})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    (schemas, markers, floors)
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture with failpoints"]
async fn channel_writer_floor_command_refuses_strict_and_unresolved_gateway_ledger() {
    let mut uri =
        url::Url::parse(&std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap()).unwrap();
    let admin = Client::with_uri_str(uri.as_str()).await.unwrap();
    let app = format!("channel_floor_{}", uuid::Uuid::new_v4().simple());
    uri.query_pairs_mut().append_pair("appName", &app);

    for strict in [true, false] {
        let db = admin.database(&format!("channel_floor_{}", uuid::Uuid::new_v4().simple()));
        let base = cli(&db, &uri, &["--migrate"]).await;
        assert!(
            base.status.success(),
            "{}",
            String::from_utf8_lossy(&base.stderr)
        );
        if strict {
            activate(&db, &uri).await;
        } else {
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":1},
                    "data":{"appName":&app,"failCommands":["createIndexes"],"errorCode":6,
                        "errorLabels":["RetryableWriteError"]},
                })
                .await
                .unwrap();
            let replica = cli(&db, &uri, &["--migrate-replicas"]).await;
            assert!(!replica.status.success());
        }
        let lock = db
            .collection::<Document>(LEDGER)
            .find_one(doc! {"_id":"runner"})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lock.get_bool("strict").unwrap(), strict);
        assert_eq!(lock.get_bool("pending").unwrap(), !strict);
        let before = snapshot(&db).await;
        let floor = cli(
            &db,
            &uri,
            &[
                "--migrate-channel-create-fence-floor",
                "--all-writers-drained",
            ],
        )
        .await;
        assert!(!floor.status.success());
        assert!(String::from_utf8_lossy(&floor.stderr)
            .contains("migration lease held or an interrupted effect requires reconciliation"));
        assert_eq!(snapshot(&db).await, before);
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn channel_writer_floor_command_records_a_completed_gateway_attempt() {
    let uri =
        url::Url::parse(&std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap()).unwrap();
    let admin = Client::with_uri_str(uri.as_str()).await.unwrap();
    let db = admin.database(&format!("channel_floor_{}", uuid::Uuid::new_v4().simple()));
    let base = cli(&db, &uri, &["--migrate"]).await;
    assert!(
        base.status.success(),
        "{}",
        String::from_utf8_lossy(&base.stderr)
    );
    let floor = cli(
        &db,
        &uri,
        &[
            "--migrate-channel-create-fence-floor",
            "--all-writers-drained",
        ],
    )
    .await;
    assert!(
        floor.status.success(),
        "{}",
        String::from_utf8_lossy(&floor.stderr)
    );
    let attempt = db
        .collection::<Document>(LEDGER)
        .find_one(doc! {"kind":"attempt","migration":"gateway-channel-create-fence-floor-v1"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(attempt.get_str("outcome").unwrap(), "completed");
    assert!(attempt.get_i64("effects").unwrap() > 0);
    let plan = cli(
        &db,
        &uri,
        &["--migration-plan", "gateway-channel-create-fence-floor-v1"],
    )
    .await;
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    assert_eq!(plan["postcondition_ready"], true);
    assert_eq!(plan["writes"], false);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn channel_writer_floor_requires_drain_and_refuses_unknown_catalog_before_phase_change() {
    let uri =
        url::Url::parse(&std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap()).unwrap();
    let admin = Client::with_uri_str(uri.as_str()).await.unwrap();
    let db = admin.database(&format!("channel_floor_{}", uuid::Uuid::new_v4().simple()));
    let base = cli(&db, &uri, &["--migrate"]).await;
    assert!(base.status.success());
    let before = snapshot(&db).await;
    let missing_legacy = cli(&db, &uri, &["--migrate-channel-create-fence-floor"]).await;
    assert!(!missing_legacy.status.success());
    assert!(String::from_utf8_lossy(&missing_legacy.stderr).contains("--all-writers-drained"));
    let missing_explicit = cli(
        &db,
        &uri,
        &[
            "--migration-apply",
            "gateway-channel-create-fence-floor-v1",
            "fixture-checksum",
            "fixture-image",
        ],
    )
    .await;
    assert!(!missing_explicit.status.success());
    assert!(String::from_utf8_lossy(&missing_explicit.stderr)
        .contains("Channel create fence floor requires --all-writers-drained"));
    assert_eq!(snapshot(&db).await, before);

    db.run_command(doc! {
        "collMod":cmaeg::records::mongo::RECORDS,
        "validator":{"unexpected":{"$exists":true}},
        "validationLevel":"strict",
        "validationAction":"error",
    })
    .await
    .unwrap();
    let drifted = snapshot(&db).await;
    let refused = cli(
        &db,
        &uri,
        &[
            "--migrate-channel-create-fence-floor",
            "--all-writers-drained",
        ],
    )
    .await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("Gateway Channel create fence floor inspection failed"));
    assert_eq!(snapshot(&db).await, drifted);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn channel_writer_floor_refuses_extra_fence_index_and_missing_required_collection() {
    let uri =
        url::Url::parse(&std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap()).unwrap();
    let admin = Client::with_uri_str(uri.as_str()).await.unwrap();
    let seed = admin.database(&format!(
        "channel_floor_seed_{}",
        uuid::Uuid::new_v4().simple()
    ));
    assert!(cli(&seed, &uri, &["--migrate"]).await.status.success());
    let floor_args = [
        "--migrate-channel-create-fence-floor",
        "--all-writers-drained",
    ];
    assert!(cli(&seed, &uri, &floor_args).await.status.success());
    let validator = seed
        .list_collections()
        .filter(doc! {"name":FENCES})
        .await
        .unwrap()
        .try_next()
        .await
        .unwrap()
        .unwrap()
        .options
        .validator
        .unwrap();
    seed.drop().await.unwrap();

    for extra_index in [true, false] {
        let db = admin.database(&format!("channel_floor_{}", uuid::Uuid::new_v4().simple()));
        assert!(cli(&db, &uri, &["--migrate"]).await.status.success());
        if extra_index {
            db.run_command(doc! {
                "create":FENCES,"validator":&validator,
                "validationLevel":"strict","validationAction":"error",
            })
            .await
            .unwrap();
            db.run_command(doc! {
                "createIndexes":FENCES,"indexes":[{"key":{"revision":1},"name":"unexpected_revision"}],
            }).await.unwrap();
        } else {
            db.collection::<Document>(cmaeg::records::mongo::RECORDS)
                .drop()
                .await
                .unwrap();
        }
        let before = snapshot(&db).await;
        assert!(before.2.is_empty());
        assert!(before.1.iter().all(|row| !matches!(
            row.get_str("_id"),
            Ok("channel_validator_change" | "channel_create_fences")
        )));
        let refused = cli(&db, &uri, &floor_args).await;
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr)
            .contains("Gateway Channel create fence floor inspection failed"));
        assert_eq!(snapshot(&db).await, before);
        db.drop().await.unwrap();
    }
}
