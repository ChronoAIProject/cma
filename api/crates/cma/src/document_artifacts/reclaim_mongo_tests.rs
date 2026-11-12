use super::{quota::*, reclaim::MongoDocumentReclaimStore, *};
use layer2_core::{document_artifacts::*, Authorization, Caller, Capability, SubjectId};
use mongodb::{
    bson::{doc, DateTime, Document},
    Client, Database,
};
use std::sync::Arc;

async fn fixture() -> (Database, MongoDocumentQuotaStore, MongoDocumentReclaimStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set required");
    let db = Client::with_uri_str(uri).await.unwrap().database(&format!(
        "cma_document_reclaim_{}",
        uuid::Uuid::new_v4().simple()
    ));
    MongoDocumentStore::new(db.clone())
        .ensure_indexes()
        .await
        .unwrap();
    let quota = MongoDocumentQuotaStore::new(&db, true);
    let activation = uuid::Uuid::new_v4().to_string();
    quota.prepare(&activation).await.unwrap();
    let inventory = super::quota_baseline::Inventory::parse(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version":1, "activation_id":activation, "complete_storage_inventory":true,
            "writers_drained":true, "objects":[]
        }))
        .unwrap(),
    )
    .unwrap();
    quota.install(&db, &inventory, true).await.unwrap();
    let reclaim = MongoDocumentReclaimStore::new(&db, true);
    (db, quota, reclaim)
}

async fn record() -> StoredDocument {
    let rows = Arc::new(memory::MemoryDocuments::default());
    let service = DocumentService::new(
        rows.clone(),
        Arc::new(memory::MemoryDocumentBlobs::default()),
    );
    let caller = Caller::new(
        SubjectId::new("reclaim-owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let metadata = service
        .create(
            &caller,
            "create",
            CreateDocument {
                title: "Document".into(),
                markdown: "source".into(),
                origin_agent_id: None,
            },
        )
        .await
        .unwrap();
    rows.get(&metadata.id).await.unwrap().unwrap()
}

fn source(record: &StoredDocument, bytes: &[u8]) -> DocumentSourceReservation {
    DocumentSourceReservation {
        owner_hash: digest(record.metadata.owner_subject.as_bytes()),
        document_id: record.metadata.id.clone(),
        sha256: digest(bytes),
        bytes: bytes.len() as u64,
    }
}
fn object_id(source: &DocumentSourceReservation) -> String {
    object_row(source).unwrap().get_str("_id").unwrap().into()
}
async fn age(quota: &MongoDocumentQuotaStore, source: &DocumentSourceReservation) {
    quota
        .rows
        .update_one(
            doc! {"_id":object_id(source)},
            doc! {"$set":{"reserved_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
}
async fn candidate(reclaim: &MongoDocumentReclaimStore) -> ObjectCandidate {
    reclaim.candidates(10_000, 64).await.unwrap().remove(0)
}
async fn owner(quota: &MongoDocumentQuotaStore, source: &DocumentSourceReservation) -> Document {
    quota
        .rows
        .find_one(doc! {"_id":owner_id(&source.owner_hash)})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_writer_commit_and_judgement_serialize_without_dangling_reference() {
    let (db, quota, reclaim) = fixture().await;
    let metadata = record().await;
    let reservation = source(&metadata, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    assert_eq!(reclaim.judge(&c).await.unwrap(), Judgement::Condemned);
    let store = MongoDocumentStore::new(db.clone());
    assert_eq!(
        store
            .insert(&metadata, Some((&reservation, SourceTicket::Ledgered)))
            .await,
        Err(DocumentError::Unavailable)
    );
    assert!(store.get(&metadata.metadata.id).await.unwrap().is_none());
    assert!(reclaim.finalize(&c).await.unwrap());
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    assert!(store
        .insert(&metadata, Some((&reservation, SourceTicket::Ledgered)))
        .await
        .unwrap());
    assert!(matches!(
        reclaim.judge(&c).await.unwrap(),
        Judgement::Skipped | Judgement::Committed
    ));
    assert!(!reclaim.finalize(&c).await.unwrap());
    assert!(store.get(&metadata.metadata.id).await.unwrap().is_some());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_release_is_exactly_once_across_two_stores() {
    let (db, quota, reclaim) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    assert_eq!(reclaim.judge(&c).await.unwrap(), Judgement::Condemned);
    let other = MongoDocumentReclaimStore::new(&db, true);
    let (a, b) = tokio::join!(reclaim.finalize(&c), other.finalize(&c));
    assert_eq!(u8::from(a.unwrap()) + u8::from(b.unwrap()), 1);
    let usage = owner(&quota, &reservation).await;
    for key in ["bytes", "objects", "documents"] {
        assert_eq!(usage.get_i64(key).unwrap(), 0);
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_finalize_serializes_with_reserve_on_owner_fence() {
    let (db, quota, reclaim) = fixture().await;
    let mut reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    reclaim.judge(&c).await.unwrap();
    reservation.sha256 = digest(b"next");
    reservation.bytes = 4;
    let (released, reserved) = tokio::join!(reclaim.finalize(&c), quota.reserve(&reservation));
    assert!(released.unwrap());
    assert_eq!(reserved.unwrap(), SourceTicket::Ledgered);
    let usage = owner(&quota, &reservation).await;
    assert_eq!(usage.get_i64("bytes").unwrap(), 4);
    assert_eq!(usage.get_i64("objects").unwrap(), 1);
    assert_eq!(usage.get_i64("documents").unwrap(), 1);
    assert!(usage.get_i64("fence").unwrap() >= 3);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_condemned_retry_is_refused_until_finalized_then_reserves_fresh() {
    let (db, quota, reclaim) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    reclaim.judge(&c).await.unwrap();
    assert_eq!(
        quota.reserve(&reservation).await,
        Err(DocumentError::Unavailable)
    );
    assert!(reclaim.finalize(&c).await.unwrap());
    assert_eq!(
        quota.reserve(&reservation).await.unwrap(),
        SourceTicket::Ledgered
    );
    let row = quota
        .rows
        .find_one(doc! {"_id":object_id(&reservation)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_str("state").unwrap(), "reserved");
    assert!(row.get_datetime("reserved_at").unwrap().timestamp_millis() > 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_candidates_use_the_state_index() {
    let (db, quota, reclaim) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    assert_eq!(reclaim.candidates(10_000, 64).await.unwrap().len(), 1);
    let explain = db.run_command(doc!{"explain":{"find":QUOTA_COLLECTION,
        "filter":{"kind":"object","state":"reserved","reserved_at":{"$lt":DateTime::from_millis(10_000)}},
        "hint":{"kind":1,"state":1,"reserved_at":1}}, "verbosity":"queryPlanner"}).await.unwrap();
    let plan = serde_json::to_string(explain.get_document("queryPlanner").unwrap()).unwrap();
    assert!(plan.contains("IXSCAN"));
    assert!(plan.contains("kind_1_state_1_reserved_at_1"));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_legacy_time_fallback_repairs_referenced_baseline() {
    let (db, quota, reclaim) = fixture().await;
    let metadata = record().await;
    let reservation = source(&metadata, b"source");
    quota.reserve(&reservation).await.unwrap();
    MongoDocumentStore::new(db.clone())
        .insert(&metadata, None)
        .await
        .unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id":object_id(&reservation)},
            doc! {"$unset":{"state":"","reserved_at":""}},
        )
        .await
        .unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id":STATE},
            doc! {"$set":{"activated_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    let c = candidate(&reclaim).await;
    assert_eq!(reclaim.judge(&c).await.unwrap(), Judgement::Committed);
    assert!(!reclaim.finalize(&c).await.unwrap());
    quota
        .rows
        .update_one(
            doc! {"_id":object_id(&reservation)},
            doc! {"$unset":{"state":""}},
        )
        .await
        .unwrap();
    quota
        .rows
        .update_one(doc! {"_id":STATE}, doc! {"$unset":{"activated_at":""}})
        .await
        .unwrap();
    assert!(reclaim.candidates(10_000, 64).await.unwrap().is_empty());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_release_never_makes_counters_negative() {
    let (db, quota, reclaim) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    reclaim.judge(&c).await.unwrap();
    quota
        .rows
        .update_one(
            doc! {"_id":owner_id(&reservation.owner_hash)},
            doc! {"$set":{"bytes":0_i64}},
        )
        .await
        .unwrap();
    assert_eq!(reclaim.finalize(&c).await, Err(DocumentError::Corrupt));
    assert!(quota
        .rows
        .find_one(doc! {"_id":object_id(&reservation)})
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        owner(&quota, &reservation).await.get_i64("bytes").unwrap(),
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_candidates_prioritize_condemned_and_recheck_age() {
    let (db, quota, reclaim) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let c = candidate(&reclaim).await;
    quota
        .rows
        .update_one(
            doc! {"_id":object_id(&reservation)},
            doc! {"$set":{"reserved_at":DateTime::from_millis(20_000)}},
        )
        .await
        .unwrap();
    assert_eq!(reclaim.judge(&c).await.unwrap(), Judgement::Skipped);
    age(&quota, &reservation).await;
    reclaim.judge(&c).await.unwrap();
    let recent = reclaim.candidates(0, 1).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert!(recent[0].condemned);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_legacy_partial_baseline_resumes_without_inventing_baseline_usage() {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set required");
    let db = Client::with_uri_str(uri).await.unwrap().database(&format!(
        "cma_document_reclaim_{}",
        uuid::Uuid::new_v4().simple()
    ));
    MongoDocumentStore::new(db.clone())
        .ensure_indexes()
        .await
        .unwrap();
    let quota = MongoDocumentQuotaStore::new(&db, true);
    let activation = uuid::Uuid::new_v4().to_string();
    quota.prepare(&activation).await.unwrap();
    let reservation = source(&record().await, b"source");
    let inventory = super::quota_baseline::Inventory::parse(
        &serde_json::to_vec(&serde_json::json!({
            "schema_version":1,"activation_id":activation,"complete_storage_inventory":true,
            "writers_drained":true,"objects":[reservation]
        }))
        .unwrap(),
    )
    .unwrap();
    quota
        .rows
        .insert_one(
            doc! {"_id":owner_id(&reservation.owner_hash),"document_schema_version":1_i64,
            "kind":"owner","fence":0_i64,"documents":1_i64,"bytes":6_i64,"objects":1_i64},
        )
        .await
        .unwrap();
    quota.install(&db, &inventory, true).await.unwrap();
    let usage = owner(&quota, &reservation).await;
    assert!(!usage.contains_key("baseline_documents"));
    let marker = quota
        .rows
        .find_one(doc! {"_id":STATE})
        .await
        .unwrap()
        .unwrap();
    assert!(marker.get_datetime("activated_at").is_ok());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_stale_candidate_never_releases_a_new_reservation_incarnation() {
    for legacy in [false, true] {
        let (db, quota, reclaim) = fixture().await;
        let reservation = source(&record().await, b"source");
        quota.reserve(&reservation).await.unwrap();
        if legacy {
            quota
                .rows
                .update_one(
                    doc! {"_id":object_id(&reservation)},
                    doc! {"$unset":{"reservation_id":""}},
                )
                .await
                .unwrap();
        }
        age(&quota, &reservation).await;
        let old = candidate(&reclaim).await;
        reclaim.judge(&old).await.unwrap();
        assert!(reclaim.finalize(&old).await.unwrap());
        quota.reserve(&reservation).await.unwrap();
        age(&quota, &reservation).await;
        let new = candidate(&reclaim).await;
        assert_ne!(old.reservation_id, new.reservation_id);
        assert_eq!(reclaim.judge(&old).await.unwrap(), Judgement::Skipped);
        assert_eq!(reclaim.judge(&new).await.unwrap(), Judgement::Condemned);
        assert!(!reclaim.finalize(&old).await.unwrap());
        let usage = owner(&quota, &reservation).await;
        assert_eq!(usage.get_i64("objects").unwrap(), 1);
        assert_eq!(usage.get_i64("bytes").unwrap(), 6);
        assert!(reclaim.finalize(&new).await.unwrap());
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_reclaim_unknown_commits_use_snapshot_reconciliation_without_double_release() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (db, quota, _) = fixture().await;
    let reservation = source(&record().await, b"source");
    quota.reserve(&reservation).await.unwrap();
    age(&quota, &reservation).await;
    let application = format!("reclaim-unknown-{}", uuid::Uuid::new_v4().simple());
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(application.clone());
    let snapshots = Arc::new(AtomicUsize::new(0));
    let observed = snapshots.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find"
                && !event.command.contains_key("autocommit")
                && event
                    .command
                    .get_document("readConcern")
                    .ok()
                    .is_some_and(|concern| concern.get_str("level").ok() == Some("snapshot"))
            {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let reclaim = MongoDocumentReclaimStore::new(&client.database(db.name()), true);
    let c = candidate(&reclaim).await;
    let admin = db.client().database("admin");
    let injection = doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},
    "data":{"failCommands":["commitTransaction"],"appName":application,
    "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected reclaim commit ambiguity"}}};
    admin.run_command(injection.clone()).await.unwrap();
    let judged = reclaim.judge(&c).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(judged.unwrap(), Judgement::Condemned);
    assert!(
        snapshots.load(Ordering::SeqCst) > 0,
        "unknown judgement must reconcile through a snapshot"
    );
    let before = snapshots.load(Ordering::SeqCst);
    admin.run_command(injection).await.unwrap();
    let released = reclaim.finalize(&c).await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(
        !released.unwrap(),
        "ambiguous release is not attributed to this process"
    );
    assert!(
        snapshots.load(Ordering::SeqCst) > before,
        "unknown release must reconcile through a snapshot"
    );
    assert!(!reclaim.finalize(&c).await.unwrap());
    let usage = owner(&quota, &reservation).await;
    for field in ["documents", "bytes", "objects"] {
        assert_eq!(usage.get_i64(field).unwrap(), 0);
    }
    db.drop().await.unwrap();
}
