use super::*;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use mongodb::bson::{DateTime, Document};

mod activity_page;
mod collection;
mod failpoints;
mod key_order;
mod measured;
mod opening;
mod release_identity;
mod segments;
mod segments_runtime;
mod source_keys;
pub(super) mod startup_ddl;
mod usage_capture;

fn identity() -> ledger::Identity {
    ledger::Identity::new(
        &"a".repeat(40),
        &format!("ghcr.io/chronoaiproject/cma-api@sha256:{}", "b".repeat(64)),
    )
    .unwrap()
}

#[test]
fn additive_indexes_accept_exact_existing_state_and_refuse_semantic_drift() {
    let indexes = activity_indexes::indexes();
    assert_eq!(activity_indexes::missing(&[]).unwrap().len(), 2);
    assert!(activity_indexes::missing(&indexes).unwrap().is_empty());
    assert_eq!(
        activity_indexes::missing(&indexes[..1]).unwrap(),
        indexes[1..]
    );
    let mutations: [(&str, mongodb::bson::Bson); 8] = [
        ("unique", true.into()),
        ("sparse", true.into()),
        ("hidden", true.into()),
        ("expireAfterSeconds", 1_i64.into()),
        ("partialFilterExpression", doc! {"present":true}.into()),
        ("collation", doc! {"locale":"en"}.into()),
        ("name", "another-index".into()),
        ("key", doc! {"record.actor":-1}.into()),
    ];
    for (key, value) in mutations {
        let mut changed = indexes.to_vec();
        changed[0].insert(key, value);
        assert!(activity_indexes::missing(&changed).is_err(), "{key}");
    }
    let mut duplicate = indexes.to_vec();
    duplicate.push(indexes[0].clone());
    assert!(activity_indexes::missing(&duplicate).is_err());
    let mut collated = indexes.to_vec();
    collated.push(doc! {"name":"_id_","key":{"_id":1},"collation":{"locale":"en"}});
    assert!(activity_indexes::missing(&collated).is_err());
}

#[test]
fn migration_identity_refuses_tags_other_services_and_unstamped_builds() {
    identity();
    assert!(ledger::Identity::new(
        &"a".repeat(40),
        &format!(
            "us-central1-docker.pkg.dev/example-project-01/example-registry-02/cma-api@sha256:{}",
            "b".repeat(64)
        ),
    )
    .is_ok());
    for (source, image) in [
        (
            "development".to_owned(),
            format!("ghcr.io/chronoaiproject/cma-api@sha256:{}", "b".repeat(64)),
        ),
        (
            "a".repeat(40),
            "ghcr.io/chronoaiproject/cma-api:current".into(),
        ),
        (
            "a".repeat(40),
            format!(
                "ghcr.io/chronoaiproject/cma-cmaeg@sha256:{}",
                "b".repeat(64)
            ),
        ),
    ] {
        assert!(ledger::Identity::new(&source, &image).is_err());
    }
    assert_eq!(activity_indexes::checksum().len(), 64);
}

#[test]
fn migration_authority_never_grants_cross_service_or_destructive_actions() {
    let role = authority::role("fixture", true);
    let status = |privileges| {
        doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
        "authenticatedUserPrivileges":privileges}}
    };
    let grants = role.get_array("privileges").unwrap().clone();
    for mode in [
        MongoAuthorityMode::Isolated,
        MongoAuthorityMode::SharedSchema,
    ] {
        assert!(authority::verify(&status(grants.clone()), "fixture", mode, true).is_ok());
    }
    let shared = status(vec![doc! {"resource":{"db":"fixture","collection":""},
    "actions":["find","insert","update","createCollection","createIndex","listIndexes"]}
    .into()]);
    assert!(authority::verify(&shared, "fixture", MongoAuthorityMode::SharedSchema, true).is_ok());
    assert!(authority::verify(&shared, "fixture", MongoAuthorityMode::Isolated, true).is_err());
    for action in ["remove", "dropCollection", "dropIndex", "collMod"] {
        let mut changed = grants.clone();
        changed[0]
            .as_document_mut()
            .unwrap()
            .get_array_mut("actions")
            .unwrap()
            .push(action.into());
        assert!(authority::verify(
            &status(changed),
            "fixture",
            MongoAuthorityMode::Isolated,
            true
        )
        .is_err());
    }
    let read = authority::role("fixture", false)
        .get_array("privileges")
        .unwrap()
        .clone();
    assert!(authority::verify(
        &status(read.clone()),
        "fixture",
        MongoAuthorityMode::Isolated,
        false
    )
    .is_ok());
    assert!(
        authority::verify(&status(read), "fixture", MongoAuthorityMode::Isolated, true).is_err()
    );
}

async fn fixture() -> (Client, mongodb::Database, ledger::Ledger) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI required");
    let client = Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!("cma_migration_{}", uuid::Uuid::new_v4().simple()));
    let ledger = ledger::Ledger::new(client.clone(), database.clone());
    (client, database, ledger)
}

async fn expire(database: &mongodb::Database) {
    database
        .collection::<Document>(ledger::COLLECTION)
        .update_one(
            doc! {"_id":"runner"},
            doc! {"$set":{"expires_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn plan_is_read_only_and_apply_reruns_without_new_effects() {
    let (_, database, ledger) = fixture().await;
    ledger.preflight().await.unwrap();
    assert!(!ledger.completed().await.unwrap());
    assert_eq!(activity_indexes::inspect(&database).await.unwrap().len(), 2);
    assert!(database.list_collection_names().await.unwrap().is_empty());
    let saved = doc! {"_id":"retained","record":{"actor":"fixture","action":"unchanged"}};
    database
        .collection("activity")
        .insert_one(saved.clone())
        .await
        .unwrap();
    let result = apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(result["outcome"], "completed");
    assert_eq!(result["created_indexes"], 2);
    assert_eq!(
        apply_indexes(&ledger, &database, &identity())
            .await
            .unwrap()["outcome"],
        "already_completed"
    );
    assert_eq!(
        database
            .collection::<Document>("activity")
            .find_one(doc! {"_id":"retained"})
            .await
            .unwrap(),
        Some(saved)
    );
    assert_eq!(
        database
            .collection::<Document>(ledger::COLLECTION)
            .count_documents(doc! {"kind":"attempt"})
            .await
            .unwrap(),
        1
    );
    assert!(ledger.completed().await.unwrap());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn concurrent_executor_and_expired_executor_cannot_publish_completion() {
    let (client, database, ledger) = fixture().await;
    ledger.initialize().await.unwrap();
    let other = ledger::Ledger::new(client, database.clone());
    let first_identity = identity();
    let other_identity = identity();
    let (left, right) = tokio::join!(ledger.claim(&first_identity), other.claim(&other_identity));
    assert_ne!(left.is_ok(), right.is_ok());
    let first = left.or(right).unwrap();
    ledger.renew(&first).await.unwrap();
    expire(&database).await;
    let second = other.claim(&identity()).await.unwrap();
    assert!(second.fence > first.fence);
    assert!(ledger.renew(&first).await.is_err());
    assert!(ledger.finish(&first, Ok(0)).await.is_err());
    other
        .finish(&second, Err("fixture interruption"))
        .await
        .unwrap();
    assert!(!ledger.completed().await.unwrap());
    assert!(other.finish(&second, Ok(0)).await.is_err());
    let statuses = ledger.status().await.unwrap();
    let attempts = statuses["recent_attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(attempts.iter().any(|row| row["outcome"] == "failed"));
    assert!(attempts.iter().any(|row| row["outcome"] == "running"));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn interrupted_partial_ddl_resumes_and_keeps_prior_attempt() {
    let (_, database, ledger) = fixture().await;
    ledger.initialize().await.unwrap();
    let abandoned = ledger.claim(&identity()).await.unwrap();
    activity_indexes::create(&database, activity_indexes::indexes()[0].clone())
        .await
        .unwrap();
    expire(&database).await;
    let result = apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    assert_eq!(result["created_indexes"], 1);
    assert!(activity_indexes::inspect(&database)
        .await
        .unwrap()
        .is_empty());
    assert!(ledger.finish(&abandoned, Ok(2)).await.is_err());
    assert_eq!(
        ledger.status().await.unwrap()["recent_attempts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn checksum_unknown_history_and_missing_completion_receipt_fail_closed() {
    let (_, database, ledger) = fixture().await;
    ledger.initialize().await.unwrap();
    let rows = database.collection::<Document>(ledger::COLLECTION);
    rows.update_one(doc! {"kind":"manifest"}, doc! {"$set":{"checksum":"drift"}})
        .await
        .unwrap();
    assert!(ledger.preflight().await.is_err());
    assert!(apply_indexes(&ledger, &database, &identity())
        .await
        .is_err());
    assert_eq!(activity_indexes::inspect(&database).await.unwrap().len(), 2);
    rows.update_one(
        doc! {"kind":"manifest"},
        doc! {"$set":{"checksum":activity_indexes::checksum()}},
    )
    .await
    .unwrap();
    rows.insert_one(doc! {"_id":"future","schema":2_i64,"kind":"manifest"})
        .await
        .unwrap();
    assert!(ledger.preflight().await.is_err());
    assert!(ledger.claim(&identity()).await.is_err());
    rows.delete_one(doc! {"_id":"future"}).await.unwrap();
    apply_indexes(&ledger, &database, &identity())
        .await
        .unwrap();
    rows.delete_many(doc! {"kind":"attempt"}).await.unwrap();
    assert_eq!(
        ledger.completed().await.unwrap_err(),
        "completion receipt missing"
    );
    database.drop().await.unwrap();
}
