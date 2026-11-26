use super::*;
use crate::document_artifacts::quota_baseline::Inventory;
use std::sync::Arc;

#[path = "reclamation_mongo_tests.rs"]
mod reclamation;
#[path = "source_commit_mongo_tests.rs"]
mod source_commit;

async fn database() -> Database {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo replica set required");
    let database = Client::with_uri_str(uri)
        .await
        .unwrap()
        .database(&format!("cma_quota_test_{}", uuid::Uuid::new_v4().simple()));
    database.create_collection(QUOTA_COLLECTION).await.unwrap();
    database
}
fn source(owner: &str, id: u32, body: &[u8]) -> DocumentSourceReservation {
    DocumentSourceReservation {
        owner_hash: digest(owner.as_bytes()),
        document_id: format!("doc_{id:032x}"),
        sha256: digest(body),
        bytes: body.len() as u64,
    }
}
fn inventory(id: &str, sources: &[DocumentSourceReservation]) -> Inventory {
    Inventory::parse(
        &serde_json::to_vec(
            &serde_json::json!({"schema_version": 1, "activation_id": id,
        "complete_storage_inventory": true, "writers_drained": true, "objects": sources}),
        )
        .unwrap(),
    )
    .unwrap()
}
async fn activate(
    store: &MongoDocumentQuotaStore,
    database: &Database,
    sources: &[DocumentSourceReservation],
) -> Inventory {
    let id = uuid::Uuid::new_v4().to_string();
    store.prepare(&id).await.unwrap();
    let inventory = inventory(&id, sources);
    store.install(database, &inventory, true).await.unwrap();
    inventory
}
async fn usage(store: &MongoDocumentQuotaStore, owner: &str) -> [u64; 3] {
    let row = store
        .rows
        .find_one(doc! {"_id": owner_id(&digest(owner.as_bytes()))})
        .await
        .unwrap()
        .unwrap();
    [
        number(&row, "documents").unwrap(),
        number(&row, "bytes").unwrap(),
        number(&row, "objects").unwrap(),
    ]
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_cross_process_races_never_overshoot_and_reopen_preserves_exact_retries() {
    let database = database().await;
    let mut a = MongoDocumentQuotaStore::new(&database, true);
    a.limits = [3, 12, 3];
    let mut b = MongoDocumentQuotaStore::new(&database, true);
    b.limits = a.limits;
    let a = Arc::new(a);
    let b = Arc::new(b);
    activate(&a, &database, &[]).await;
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..12 {
        let store = if n % 2 == 0 { a.clone() } else { b.clone() };
        tasks.spawn(async move {
            let source = source("owner", n, b"body");
            (source.clone(), store.reserve(&source).await)
        });
    }
    let mut admitted = Vec::new();
    let mut refused = 0;
    while let Some(result) = tasks.join_next().await {
        let (source, result) = result.unwrap();
        match result {
            Ok(()) => admitted.push(source),
            Err(DocumentError::Capacity) => refused += 1,
            other => panic!("unexpected admission: {other:?}"),
        }
    }
    assert_eq!((admitted.len(), refused), (3, 9));
    assert_eq!(usage(&a, "owner").await, [3, 12, 3]);
    let mut reopened = MongoDocumentQuotaStore::new(&database, true);
    reopened.limits = [3, 12, 3];
    for source in &admitted {
        reopened.reserve(source).await.unwrap();
    }
    assert_eq!(usage(&reopened, "owner").await, [3, 12, 3]);
    reopened
        .reserve(&source("other-owner", 99, b"body"))
        .await
        .unwrap();
    assert_eq!(usage(&reopened, "other-owner").await, [1, 4, 1]);
    assert_eq!(
        reopened.reserve(&source("owner", 98, b"body")).await,
        Err(DocumentError::Capacity)
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_count_bytes_and_tiny_object_limits_are_independent_and_deduplicated() {
    let database = database().await;
    let mut store = MongoDocumentQuotaStore::new(&database, true);
    store.limits = [1, 6, 2];
    activate(&store, &database, &[]).await;
    let original = source("owner", 1, b"body");
    store.reserve(&original).await.unwrap();
    store.reserve(&original).await.unwrap();
    assert_eq!(
        store.reserve(&source("owner", 2, b"x")).await,
        Err(DocumentError::Capacity)
    );
    assert_eq!(
        store.reserve(&source("owner", 1, b"new")).await,
        Err(DocumentError::Capacity)
    );
    store.reserve(&source("owner", 1, b"y")).await.unwrap();
    assert_eq!(
        store.reserve(&source("owner", 1, b"z")).await,
        Err(DocumentError::Capacity)
    );
    assert_eq!(usage(&store, "owner").await, [1, 5, 2]);
    assert_eq!(
        store
            .reserve(&DocumentSourceReservation {
                bytes: 3,
                ..original
            })
            .await,
        Err(DocumentError::Corrupt)
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_default_off_unknown_baseline_and_disabled_after_preparation_fail_safely() {
    let database = database().await;
    let off = MongoDocumentQuotaStore::new(&database, false);
    let on = MongoDocumentQuotaStore::new(&database, true);
    let object = source("owner", 1, b"body");
    off.reserve(&object).await.unwrap();
    assert_eq!(off.rows.count_documents(doc! {}).await.unwrap(), 0);
    assert_eq!(on.reserve(&object).await, Err(DocumentError::Unavailable));
    let id = uuid::Uuid::new_v4().to_string();
    on.prepare(&id).await.unwrap();
    for store in [&off, &on] {
        assert_eq!(
            store.reserve(&object).await,
            Err(DocumentError::Unavailable)
        );
    }
    let inventory = inventory(&id, &[]);
    on.install(&database, &inventory, true).await.unwrap();
    on.reserve(&object).await.unwrap();
    assert_eq!(off.reserve(&object).await, Err(DocumentError::Unavailable));
    on.install(&database, &inventory, true).await.unwrap();
    assert_eq!(usage(&on, "owner").await, [1, 4, 1]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_failed_reservation_transaction_rolls_back_all_counters_and_rows() {
    let database = database().await;
    let store = MongoDocumentQuotaStore::new(&database, true);
    activate(&store, &database, &[]).await;
    database.run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {"kind": {"$ne": "object"}}, "validationLevel": "strict"}).await.unwrap();
    assert_eq!(
        store.reserve(&source("owner", 1, b"body")).await,
        Err(DocumentError::Unavailable)
    );
    assert_eq!(store.rows.count_documents(doc! {}).await.unwrap(), 1);
    database
        .run_command(doc! {"collMod": QUOTA_COLLECTION, "validator": {}})
        .await
        .unwrap();
    store.reserve(&source("owner", 1, b"body")).await.unwrap();
    assert_eq!(usage(&store, "owner").await, [1, 4, 1]);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_baseline_retains_over_limit_orphans_and_refuses_changed_inventory() {
    let database = database().await;
    let mut store = MongoDocumentQuotaStore::new(&database, true);
    store.limits = [1, 4, 1];
    let original = source("owner", 1, b"body");
    let orphan = source("owner", 2, b"orphan");
    let inventory = activate(&store, &database, &[original.clone(), orphan.clone()]).await;
    assert_eq!(usage(&store, "owner").await, [2, 10, 2]);
    store.reserve(&orphan).await.unwrap();
    assert_eq!(
        store.reserve(&source("owner", 1, b"new")).await,
        Err(DocumentError::Capacity)
    );
    let changed = self::inventory(&inventory.activation, &[original]);
    assert_eq!(
        store.install(&database, &changed, true).await,
        Err(DocumentError::Conflict)
    );
    assert_eq!(
        store.prepare(&uuid::Uuid::new_v4().to_string()).await,
        Err(DocumentError::Conflict)
    );
    database.drop().await.unwrap();
}

#[path = "quota_baseline_tests.rs"]
mod baseline;
#[path = "quota_effect_tests.rs"]
mod effects;
#[path = "quota_recovery_tests.rs"]
mod recovery;
