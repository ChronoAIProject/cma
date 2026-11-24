use super::{tests::*, *};
use crate::store::{legal_consents::MongoLegalConsentStore, retained_consents};
use futures_util::TryStreamExt;
use layer2_core::legal_consent::LegalConsentStore;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client,
};

const ORIGINAL: &str = "2026-11-10T12:00:00Z";
const LATEST: &str = "2026-11-13T12:00:00Z";

fn retention_options() -> Options {
    Options {
        billing_enabled: false,
        allocation_rollups_enabled: Some(false),
        agent_shared_observation: Some(false),
        retain_legal_consents: true,
        transaction_drain_minimum_secs: 120,
        drain_qualification: Some(TransactionDrainQualification::fixture()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn concurrent_acceptance_and_erasure_follow_the_committed_order() {
    use crate::store::settings_barrier::test_support::{Pause, Point};
    use std::sync::Arc;

    let db = database().await;
    let pause = Arc::new(Pause::default());
    let controlled = pause.client().await.database(db.name());
    retained_consents::prepare(&db).await.unwrap();
    user(&db, "acct:accept-first").await;
    let release = pause.arm(Point::ConsentAck);
    let consents = MongoLegalConsentStore::new(&controlled);
    let accepting = tokio::spawn(async move {
        consents
            .accept(&SubjectId::new("acct:accept-first"), "2026-11-13", LATEST)
            .await
    });
    pause.wait().await;
    assert!(retained_consents::erase_user(&db, "acct:accept-first")
        .await
        .unwrap());
    drop(release);
    assert!(accepting.await.unwrap().is_ok());
    assert_eq!(receipts(&db).await.len(), 2);

    user(&db, "acct:erase-first").await;
    let release = pause.arm(Point::CommitAck);
    let erasing = tokio::spawn(async move {
        retained_consents::erase_user(&controlled, "acct:erase-first").await
    });
    pause.wait().await;
    assert!(MongoLegalConsentStore::new(&db)
        .accept(&SubjectId::new("acct:erase-first"), "2026-11-13", LATEST)
        .await
        .is_err());
    drop(release);
    assert!(erasing.await.unwrap().unwrap());
    assert_eq!(count(&db, "users").await, 0);
    assert_eq!(receipts(&db).await.len(), 3);
    db.drop().await.unwrap();
}

pub(super) async fn user(db: &Database, id: &str) {
    put(
        db,
        "users",
        doc! {
            "_id": id,
            "document_schema_version": 1_i64,
            "display_name": "Synthetic profile",
            "legal_consents": [{ "version": "2026-11-10", "accepted_at": ORIGINAL, "extra": "discard" }],
        },
    )
    .await;
}

async fn receipts(db: &Database) -> Vec<Document> {
    db.collection::<Document>(retained_consents::COLLECTION)
        .find(doc! {})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn erasure_retains_only_version_time_and_detached_random_identity() {
    let db = database().await;
    let (first, second) = (SubjectId::new("acct:a"), SubjectId::new("acct:b"));
    user(&db, first.as_str()).await;
    user(&db, second.as_str()).await;
    let consents = MongoLegalConsentStore::new(&db);
    consents.accept(&first, "2026-11-13", LATEST).await.unwrap();
    let inventory = ErasureInventory::default();
    let targets = [
        Target {
            subject: &first,
            inventory: &inventory,
        },
        Target {
            subject: &second,
            inventory: &inventory,
        },
    ];
    let options = retention_options();
    let mut cursor = Cursor {
        test_drain_seconds: Some(0),
        ..Default::default()
    };
    let mut done = false;
    for _ in 0..2000 {
        if !erase_records_with_options(&db, &targets, &mut cursor, NOW, options)
            .await
            .unwrap()
            .has_more
        {
            done = true;
            break;
        }
    }
    assert!(done);
    assert_eq!(count(&db, "users").await, 0);
    assert!(consents.accept(&first, "2026-11-14", NOW).await.is_err());
    assert_eq!(count(&db, "users").await, 0);
    let archived = receipts(&db).await;
    assert_eq!(archived.len(), 3);
    let mut accounts = BTreeMap::<_, BTreeMap<_, _>>::new();
    for receipt in &archived {
        assert_eq!(receipt.len(), 2);
        let id = receipt.get_document("_id").unwrap();
        assert_eq!(id.len(), 2);
        let account = id.get_binary_generic("anonymous_account_id").unwrap();
        assert_eq!(account.len(), 32);
        accounts.entry(account.clone()).or_default().insert(
            id.get_str("terms_version").unwrap(),
            receipt.get_str("accepted_at").unwrap(),
        );
    }
    assert_eq!(accounts.len(), 2);
    assert!(accounts
        .values()
        .all(|history| history.get("2026-11-10") == Some(&ORIGINAL)));
    assert_eq!(
        accounts
            .values()
            .filter(|history| history.get("2026-11-13") == Some(&LATEST))
            .count(),
        1
    );
    assert!(!retained_consents::erase_user(&db, first.as_str())
        .await
        .unwrap());
    assert_eq!(receipts(&db).await.len(), 3);
    user(&db, first.as_str()).await;
    assert!(retained_consents::erase_user(&db, first.as_str())
        .await
        .unwrap());
    let random_ids: BTreeSet<_> = receipts(&db)
        .await
        .into_iter()
        .map(|row| {
            row.get_document("_id")
                .unwrap()
                .get_binary_generic("anonymous_account_id")
                .unwrap()
                .clone()
        })
        .collect();
    assert_eq!(
        random_ids.len(),
        3,
        "the archive must not encode a stable owner identifier"
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn directory_erasure_validates_empty_and_duplicate_histories() {
    let db = database().await;
    retained_consents::prepare(&db).await.unwrap();
    put(
        &db,
        "users",
        doc! { "_id": "acct:empty", "document_schema_version": 1_i64 },
    )
    .await;
    assert!(retained_consents::erase_user(&db, "acct:empty")
        .await
        .unwrap());
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == "users")
        .unwrap();
    let users = db.collection::<Document>("users");
    for (subject, row) in [
        (
            "acct:future",
            doc! { "_id": "acct:future", "document_schema_version": 2_i64 },
        ),
        (
            "acct:duplicate",
            doc! {
                "_id": "acct:duplicate", "document_schema_version": 1_i64,
                "legal_consents": [
                    { "version": "2026-11-10", "accepted_at": ORIGINAL },
                    { "version": "2026-11-10", "accepted_at": LATEST },
                ],
            },
        ),
    ] {
        put(&db, "users", row.clone()).await;
        let page = batches::DeletePage {
            rows: std::slice::from_ref(&row),
            ids: &[subject.into()],
        };
        let mut batch = Batch::default();
        let error = batches::remove(&db, entry, page, &[], NOW, retention_options(), &mut batch)
            .await
            .unwrap_err();
        let Error::Blocked(block) = error else {
            panic!("unexpected error: {error:?}");
        };
        assert_eq!(
            block.subject_hash,
            layer2_core::document_artifacts::digest(subject.as_bytes())
        );
        assert_eq!(
            block.reason,
            ErasureBlock::CorruptRecord {
                collection: "users".into()
            }
        );
        assert!(batch.deleted.is_empty());
        assert_eq!(
            users.find_one(doc! { "_id": subject }).await.unwrap(),
            Some(row)
        );
    }
    assert_eq!(count(&db, retained_consents::COLLECTION).await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_and_denied_receipts_abort_directory_removal() {
    let db = database().await;
    retained_consents::prepare(&db).await.unwrap();
    let users = db.collection::<Document>("users");
    let malformed = doc! {
        "_id": "acct:bad", "document_schema_version": 1_i64,
        "legal_consents": [{"version": "valid", "accepted_at": "bad timestamp"}],
    };
    put(&db, "users", malformed.clone()).await;
    assert!(matches!(
        retained_consents::erase_user(&db, "acct:bad").await,
        Err(Error::InvalidRecord("users"))
    ));
    assert_eq!(
        users.find_one(doc! {"_id":"acct:bad"}).await.unwrap(),
        Some(malformed)
    );
    db.run_command(doc! {
        "collMod": retained_consents::COLLECTION,
        "validator": { "$expr": { "$eq": [0, 1] } },
    })
    .await
    .unwrap();
    user(&db, "acct:denied").await;
    let original = users.find_one(doc! {"_id":"acct:denied"}).await.unwrap();
    assert!(matches!(
        retained_consents::erase_user(&db, "acct:denied").await,
        Err(Error::Store(_))
    ));
    assert_eq!(
        users.find_one(doc! {"_id":"acct:denied"}).await.unwrap(),
        original
    );
    assert_eq!(count(&db, retained_consents::COLLECTION).await, 0);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a replica set"]
async fn two_erasers_and_a_lost_commit_acknowledgement_publish_exactly_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(id.clone());
    let db = Client::with_options(options)
        .unwrap()
        .database(&format!("consent_{id}"));
    let peer = Client::with_uri_str(&uri)
        .await
        .unwrap()
        .database(db.name());
    user(&db, "acct:race").await;
    retained_consents::prepare(&db).await.unwrap();
    let (left, right) = tokio::join!(
        retained_consents::erase_user(&db, "acct:race"),
        retained_consents::erase_user(&peer, "acct:race"),
    );
    assert_eq!(
        usize::from(matches!(left, Ok(true))) + usize::from(matches!(right, Ok(true))),
        1
    );
    assert!(!retained_consents::erase_user(&db, "acct:race")
        .await
        .unwrap());
    user(&db, "acct:uncertain").await;
    let admin = db.client().database("admin");
    admin.run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 20_i32 },
        "data": {
            "failCommands": ["commitTransaction"], "appName": &id,
            "writeConcernError": {
                "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg":"synthetic lost acknowledgement",
            },
        },
    }).await.unwrap();
    let result = retained_consents::erase_user(&db, "acct:uncertain").await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(matches!(result, Err(Error::Store(_))), "{result:?}");
    crate::store::test_visibility::majority_barrier(peer.client(), peer.name()).await;
    assert!(!retained_consents::erase_user(&peer, "acct:uncertain")
        .await
        .unwrap());
    assert_eq!(count(&db, "users").await, 0);
    assert_eq!(count(&db, retained_consents::COLLECTION).await, 2);
    db.drop().await.unwrap();
}
