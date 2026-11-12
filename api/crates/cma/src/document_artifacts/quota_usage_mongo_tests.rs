use super::*;

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_refusal_reports_dimension_limit_and_live_without_retry_after() {
    let database = database().await;
    for (limits, dimension, limit, live) in [
        ([1, 4, 1], DocumentLimit::Documents, 1, 1),
        ([2, 6, 1], DocumentLimit::Bytes, 6, 4),
        ([2, 8, 1], DocumentLimit::Objects, 1, 1),
    ] {
        let mut store = MongoDocumentQuotaStore::new(&database, true);
        store.limits = limits;
        if store.rows.count_documents(doc! {}).await.unwrap() == 0 {
            activate(&store, &database, &[]).await;
        }
        let original = source("owner", 1, b"body");
        store.reserve(&original).await.unwrap();
        assert_eq!(
            store.reserve(&source("owner", 2, b"body")).await,
            Err(DocumentError::LimitReached {
                dimension,
                limit,
                live
            })
        );
        assert_eq!(usage(&store, "owner").await, [1, 4, 1]);
        store.reserve(&original).await.unwrap();
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_usage_reports_counters_after_reservations() {
    let database = database().await;
    let mut store = MongoDocumentQuotaStore::new(&database, true);
    store.limits = [5, 20, 8];
    activate(&store, &database, &[]).await;
    let owner = digest(b"owner");
    let initial = store.usage(&owner).await.unwrap();
    assert_eq!(initial.enforcement, QuotaEnforcement::Enforced);
    assert_eq!(initial.documents, Some(UsageCounter { used: 0, limit: 5 }));
    assert_eq!(initial.bytes, Some(UsageCounter { used: 0, limit: 20 }));
    assert_eq!(initial.objects, Some(UsageCounter { used: 0, limit: 8 }));
    let original = source("owner", 1, b"body");
    store.reserve(&original).await.unwrap();
    store.reserve(&original).await.unwrap();
    store.reserve(&source("owner", 1, b"edit")).await.unwrap();
    store.reserve(&source("owner", 2, b"new")).await.unwrap();
    store
        .reserve(&source("other-owner", 3, b"other"))
        .await
        .unwrap();
    let result = store.usage(&owner).await.unwrap();
    assert_eq!(result.documents, Some(UsageCounter { used: 2, limit: 5 }));
    assert_eq!(
        result.bytes,
        Some(UsageCounter {
            used: 11,
            limit: 20
        })
    );
    assert_eq!(result.objects, Some(UsageCounter { used: 3, limit: 8 }));
    assert_eq!(
        store
            .usage(&digest(b"other-owner"))
            .await
            .unwrap()
            .bytes
            .unwrap()
            .used,
        5
    );
    let reopened = MongoDocumentQuotaStore::new(&database, true);
    assert_eq!(
        reopened.usage(&owner).await.unwrap().bytes.unwrap().used,
        11
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_usage_is_owner_scoped_and_reports_not_enforced_without_ready_ledger() {
    let database = database().await;
    let on = MongoDocumentQuotaStore::new(&database, true);
    let off = MongoDocumentQuotaStore::new(&database, false);
    let owner = digest(b"owner");
    assert_eq!(on.usage(&owner).await, Ok(DocumentUsage::not_enforced()));
    assert_eq!(off.usage(&owner).await, Ok(DocumentUsage::not_enforced()));
    let activation = uuid::Uuid::new_v4().to_string();
    on.prepare(&activation).await.unwrap();
    assert_eq!(on.usage(&owner).await, Ok(DocumentUsage::not_enforced()));
    on.install(&database, &inventory(&activation, &[]), true)
        .await
        .unwrap();
    on.reserve(&source("owner", 1, b"body")).await.unwrap();
    assert_eq!(off.usage(&owner).await, Ok(DocumentUsage::not_enforced()));
    assert_eq!(
        on.usage(&digest(b"other-owner"))
            .await
            .unwrap()
            .documents
            .unwrap()
            .used,
        0
    );
    on.rows
        .update_one(
            doc! {"_id": owner_id(&owner)},
            doc! {"$set":{"bytes": -1_i64}},
        )
        .await
        .unwrap();
    assert_eq!(on.usage(&owner).await, Err(DocumentError::Corrupt));
    on.rows
        .update_one(doc! {"_id": STATE}, doc! {"$set":{"phase": "unknown"}})
        .await
        .unwrap();
    assert_eq!(on.usage(&owner).await, Ok(DocumentUsage::not_enforced()));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_usage_uses_bounded_majority_point_reads() {
    use mongodb::{
        event::{command::CommandEvent, EventHandler},
        options::ClientOptions,
    };
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    let commands = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                observed.lock().unwrap().push(event.command);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let database = client.database(&format!(
        "cma_quota_usage_{}",
        uuid::Uuid::new_v4().simple()
    ));
    database.create_collection(QUOTA_COLLECTION).await.unwrap();
    let store = MongoDocumentQuotaStore::new(&database, true);
    activate(&store, &database, &[]).await;
    commands.lock().unwrap().clear();
    let hash = digest(b"owner");
    store.usage(&hash).await.unwrap();
    let queries = commands.lock().unwrap().clone();
    assert_eq!(queries.len(), 2);
    for (query, id) in queries.iter().zip([STATE.to_owned(), owner_id(&hash)]) {
        assert_eq!(query.get_document("filter").unwrap(), &doc! {"_id": id});
        assert_eq!(
            query
                .get_document("readConcern")
                .unwrap()
                .get_str("level")
                .unwrap(),
            "majority"
        );
        let max_time = query.get("maxTimeMS").unwrap();
        assert_eq!(
            max_time
                .as_i64()
                .or_else(|| max_time.as_i32().map(i64::from)),
            Some(3000)
        );
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable MongoDB replica set"]
async fn document_quota_usage_failed_read_returns_unavailable() {
    use mongodb::options::ClientOptions;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uri = format!(
        "mongodb://{}/?directConnection=true",
        listener.local_addr().unwrap()
    );
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.server_selection_timeout = Some(Duration::from_millis(100));
    let client = Client::with_options(options).unwrap();
    let store = MongoDocumentQuotaStore::new(&client.database("unavailable_quota"), true);
    assert_eq!(
        store.usage(&digest(b"owner")).await,
        Err(DocumentError::Unavailable)
    );
}
