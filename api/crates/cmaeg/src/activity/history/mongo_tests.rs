use super::{
    repository::*,
    tests::{query, row},
    *,
};
use crate::{
    activity::{Change, ConsumerState},
    ingress::routing_plan::fixture,
};
use mongodb::{
    bson::{doc, Document},
    options::{ClientOptions, Credential},
    Client, Database,
};
use std::sync::Arc;

async fn database() -> (Client, Database, String) {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI")
        .expect("owned authenticated Mongo replica set required");
    let admin = Client::with_uri_str(&uri)
        .await
        .expect("fixture connection failed");
    let db = admin.database(&format!("eg_history_{}", uuid::Uuid::new_v4().simple()));
    (admin, db, uri)
}
async fn runtime(uri: &str, db: &Database) -> (Database, Database) {
    let name = db.name();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(schema::role(name, false))
        .await
        .expect("runtime role");
    db.run_command(doc! {"createUser":"history_runtime","pwd":&password,"roles":[{"role":"cma_eg_activity_history_runtime","db":name}]}).await.expect("runtime user");
    let mut options = ClientOptions::parse(uri).await.expect("runtime options");
    options.credential = Some(
        Credential::builder()
            .username("history_runtime".to_string())
            .password(password)
            .source(name.to_owned())
            .build(),
    );
    let first = Client::with_options(options.clone())
        .expect("runtime connection")
        .database(name);
    let second = Client::with_options(options)
        .expect("independent runtime connection")
        .database(name);
    (first, second)
}
async fn service(db: Database) -> ActivityHistory {
    let repo = MongoHistory::open(db).await.expect("history repository");
    ActivityHistory::new(
        Some(Arc::new(repo)),
        Arc::new(fixture::keys()),
        Config {
            enabled: true,
            capture: true,
            ..Default::default()
        },
    )
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo replica set"]
async fn mongo_two_clients_retain_55_receipts_with_frozen_pages_and_cas_updates() {
    let (_admin, db, uri) = database().await;
    schema::migrate(&db).await.unwrap();
    let (runtime, independent) = runtime(&uri, &db).await;
    schema::verify_grants(&runtime).await.unwrap();
    let a = service(runtime.clone()).await;
    let b = service(independent.clone()).await;
    let at = now_ms() - 10_000;
    schema::attest(
        &db,
        CaptureBoundary {
            revision: 2,
            retention_days: 7,
            since_ms: Some(at - 1000),
            until_ms: None,
        },
        1,
    )
    .await
    .unwrap();
    let start = std::time::Instant::now();
    for batch in 0..11 {
        let mut tasks = Vec::new();
        for offset in 0..5 {
            let service = if offset % 2 == 0 {
                a.clone()
            } else {
                b.clone()
            };
            tasks.push(tokio::spawn(async move {
                service
                    .received(
                        "owner",
                        "channel",
                        row(&format!("event-{:03}", batch * 5 + offset), at),
                    )
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }
    let elapsed = start.elapsed();
    let first = a.page("owner", "channel", query(at, 10)).await.unwrap();
    assert!(first.coverage.complete_event_enumeration);
    a.received("owner", "channel", row("late", at - 500))
        .await
        .unwrap();
    // Both duplicate writers find the same immutable first receipt.
    let (left, right) = tokio::join!(
        a.received("owner", "channel", row("event-000", at + 1000)),
        b.received("owner", "channel", row("event-000", at + 2000))
    );
    left.unwrap();
    right.unwrap();
    let one = Change::state("a", ConsumerState::Completed, Some("response-a".into()));
    let two = Change::state("b", ConsumerState::Completed, Some("response-b".into()));
    let (left, right) = tokio::join!(
        a.change("owner", "channel", "event-000", &one),
        b.change("owner", "channel", "event-000", &two)
    );
    left.unwrap();
    right.unwrap();
    let restarted = service(independent).await;
    let mut items = first.items;
    let mut cursor = first.next_cursor;
    while let Some(token) = cursor {
        let page = restarted
            .page(
                "owner",
                "channel",
                HistoryQuery {
                    cursor: Some(token),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        items.extend(page.items);
        cursor = page.next_cursor;
    }
    let identities: std::collections::BTreeSet<_> =
        items.iter().map(|i| &i.activity.event_id).collect();
    assert_eq!(identities.len(), 55);
    assert!(!identities.contains(&"late".to_string()));
    let all = super::tests::all(&restarted, query(at, 100)).await;
    let original = all
        .iter()
        .find(|i| i.activity.event_id == "event-000")
        .unwrap();
    assert_eq!(
        original.activity.received_at,
        paging::format_time(at).unwrap()
    );
    assert_eq!(original.activity.consumers.len(), 2);
    let count = db
        .collection::<Document>(schema::EVENTS)
        .count_documents(doc! {})
        .await
        .unwrap();
    assert_eq!(count, 56);
    let explain=db.run_command(doc! {"explain":{"find":schema::EVENTS,"filter":{"scope":scope("owner","channel"),"received_at":{"$gte":mongodb::bson::DateTime::from_millis(at-1000),"$lt":mongodb::bson::DateTime::from_millis(at+1000)}},"sort":{"received_at":-1,"_id":1},"hint":schema::PAGE_INDEX,"limit":11},"verbosity":"executionStats"}).await.unwrap();
    let plan = explain.to_string();
    assert!(plan.contains(schema::PAGE_INDEX));
    assert!(!plan.contains("COLLSCAN"));
    eprintln!("history Mongo qualification: 55 distinct first inserts in {}ms; 56 stored; frozen export 55; two CAS consumers",elapsed.as_millis());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo replica set"]
async fn mongo_migration_ttl_grants_and_missing_index_fail_closed() {
    let (_admin, db, uri) = database().await;
    assert!(MongoHistory::open(db.clone()).await.is_err());
    schema::migrate(&db).await.unwrap();
    schema::migrate(&db).await.unwrap();
    let (runtime, _) = runtime(&uri, &db).await;
    schema::verify_grants(&runtime).await.unwrap();
    assert!(schema::migrate(&runtime).await.is_err());
    assert!(schema::attest(
        &runtime,
        CaptureBoundary {
            revision: 2,
            retention_days: 7,
            since_ms: Some(now_ms() - 1000),
            until_ms: None
        },
        1
    )
    .await
    .is_err());
    assert!(runtime
        .collection::<Document>(schema::CHUNKS)
        .update_one(doc! {"_id":"a"}, doc! {"$set":{"blob":"changed"}})
        .await
        .is_err());
    let service = service(runtime.clone()).await;
    let at = now_ms() - 10_000;
    service
        .received("owner", "channel", row("expired", at))
        .await
        .unwrap();
    db.collection::<Document>(schema::EVENTS)
        .update_one(
            doc! {"_id":event_key(&scope("owner","channel"),"expired")},
            doc! {"$set":{"expires_at":mongodb::bson::DateTime::from_millis(now_ms()-1000)}},
        )
        .await
        .unwrap();
    let page = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    assert!(page.items.is_empty());
    db.collection::<Document>(schema::EVENTS)
        .drop_index("activity_history_expiry")
        .await
        .unwrap();
    assert!(MongoHistory::open(runtime).await.is_err());
    // Model a successor migration after removing this generation's TTL index.
    // The predecessor must refuse before recreating it or replacing validators.
    db.run_command(doc! {"collMod":schema::CONTROL,"validationLevel":"off"})
        .await
        .unwrap();
    db.collection::<Document>(schema::CONTROL)
        .update_one(
            doc! {"_id":"capture"},
            doc! {"$set":{"schema_version":2_i32}},
        )
        .await
        .unwrap();
    assert!(matches!(
        schema::migrate(&db).await,
        Err(Error::InvalidConfiguration)
    ));
    let marker = db
        .collection::<Document>(schema::CONTROL)
        .find_one(doc! {"_id":"capture"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marker.get_i32("schema_version").unwrap(), 2);
    use futures_util::TryStreamExt;
    let indexes: Vec<_> = db
        .collection::<Document>(schema::EVENTS)
        .list_indexes()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(!indexes.iter().any(|index| index
        .options
        .as_ref()
        .and_then(|options| options.name.as_deref())
        == Some("activity_history_expiry")));
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires owned authenticated disposable Mongo replica set"]
async fn mongo_large_summary_and_page_expiry_preserve_explicit_boundaries() {
    let (_admin, db, uri) = database().await;
    schema::migrate(&db).await.unwrap();
    let (runtime, _) = runtime(&uri, &db).await;
    let service = service(runtime.clone()).await;
    let at = now_ms() - 10_000;
    service
        .received("owner", "channel", row("large", at))
        .await
        .unwrap();
    for i in 0..16 {
        let mut change = Change::state(&format!("consumer-{i}"), ConsumerState::Completed, None);
        change.display_name = Some("x".repeat(5000));
        service
            .change("owner", "channel", "large", &change)
            .await
            .unwrap();
    }
    let page = service
        .page("owner", "channel", query(at, 100))
        .await
        .unwrap();
    let item = &page.items[0];
    let token = item.detail_cursor.as_ref().unwrap();
    let details = service
        .details("owner", "channel", &item.history_event_id, token)
        .await
        .unwrap();
    assert!(details.parts.len() > 1);
    assert!(!details.detail_incomplete);
    let soon = now_ms() - 7 * 86_400_000 + 3000;
    service
        .received("owner", "expiring", row("a", soon))
        .await
        .unwrap();
    service
        .received("owner", "expiring", row("b", soon + 1))
        .await
        .unwrap();
    let page = service
        .page("owner", "expiring", query(soon, 1))
        .await
        .unwrap();
    let cursor = page.next_cursor.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(3100)).await;
    assert!(matches!(
        service
            .page(
                "owner",
                "expiring",
                HistoryQuery {
                    cursor: Some(cursor),
                    ..Default::default()
                }
            )
            .await,
        Err(Error::Expired)
    ));
    db.drop().await.unwrap();
}
