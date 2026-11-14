//! Executed by the owner against one authenticated disposable replica set.
use super::*;
use mongodb::{
    bson::{doc, DateTime, Document},
    Client,
};
use std::time::Duration;

fn id() -> String {
    format!("{:064x}", uuid::Uuid::new_v4().as_u128())
}
fn limits() -> SessionLimits {
    SessionLimits {
        ttl: Duration::from_secs(600),
        idle: Duration::from_secs(60),
        max: 10,
        per_subject: 3,
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn mongo_sessions_cross_client_cas_revocation_expiry_and_caps() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI")
        .expect("authenticated disposable Mongo fixture required");
    let first = Client::with_uri_str(&uri).await.unwrap();
    let second = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_console_{}", uuid::Uuid::new_v4().simple());
    let db = first.database(&name);
    assert_eq!(schema::verify(&db).await, Err(StoreError::Unavailable));
    schema::migrate(&db).await.unwrap();
    schema::verify(&db).await.unwrap();
    let a = MongoSessionBackend::new(db.clone());
    let b = MongoSessionBackend::new(second.database(&name));
    exercise(&a, &b, &db).await;
    db.drop().await.unwrap();
}

async fn exercise(a: &MongoSessionBackend, b: &MongoSessionBackend, db: &mongodb::Database) {
    let subject = id();
    let sid = id();
    let settings = limits();
    let original = a
        .create(&sid, &subject, vec![1, 2], 90_000, &settings)
        .await
        .unwrap();
    let peer = b.load(&sid, false).await.unwrap().unwrap();
    assert_eq!(original.payload, peer.payload);
    assert_eq!(peer.subject_hash, subject);
    assert!(peer.now_ms >= original.now_ms);
    assert_eq!(original.created_ms, peer.created_ms);
    assert_eq!(original.last_seen_ms, peer.last_seen_ms);
    assert_eq!(original.expires_ms - original.created_ms, 600_000);
    assert_eq!(original.access_expires_ms - original.created_ms, 90_000);
    let (left, right) = tokio::join!(
        a.compare_exchange(&sid, &original.payload, vec![3], Some(120_000)),
        b.compare_exchange(&sid, &original.payload, vec![4], Some(120_000))
    );
    assert_eq!(
        usize::from(left.unwrap().is_some()) + usize::from(right.unwrap().is_some()),
        1
    );
    let retained = a.load(&sid, false).await.unwrap().unwrap();
    assert_eq!(
        b.revoke(&sid).await.unwrap(),
        Some(retained.payload.clone())
    );
    assert!(a.load(&sid, true).await.unwrap().is_none());
    assert!(a
        .compare_exchange(&sid, &retained.payload, vec![5], None)
        .await
        .unwrap()
        .is_none());
    assert!(db
        .collection::<Document>(schema::COLLECTION)
        .find_one(doc! {"_id":&sid})
        .await
        .unwrap()
        .is_none());
    assert_eq!(b.revoke(&sid).await.unwrap(), None);

    let expired = id();
    a.create(&expired, &subject, vec![6], 90_000, &settings)
        .await
        .unwrap();
    db.collection::<Document>(schema::COLLECTION)
        .update_one(
            doc! {"_id":&expired},
            doc! {"$set":{"expires_at":DateTime::from_millis(1)}},
        )
        .await
        .unwrap();
    assert!(b.load(&expired, false).await.unwrap().is_none());
    assert!(a
        .compare_exchange(&expired, &[6], vec![7], None)
        .await
        .unwrap()
        .is_none());
    assert!(b.load(&expired, true).await.unwrap().is_none());

    let cap = SessionLimits {
        max: 2,
        per_subject: 1,
        ..limits()
    };
    let oldest = id();
    a.create(&oldest, &subject, vec![8], 90_000, &cap)
        .await
        .unwrap();
    let newest = id();
    let admitted = b
        .create(&newest, &subject, vec![9], 90_000, &cap)
        .await
        .unwrap();
    assert_eq!(admitted.evicted.len(), 1);
    assert_eq!(admitted.evicted[0].id, oldest);
    assert_eq!(admitted.evicted[0].payload, [8]);
    assert!(a.load(&oldest, false).await.unwrap().is_none());
    assert!(a.load(&newest, false).await.unwrap().is_some());
    let other = id();
    let other_subject = id();
    a.create(&other, &other_subject, vec![10], 90_000, &cap)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    b.load(&newest, true).await.unwrap().unwrap();
    a.create(&id(), &id(), vec![11], 90_000, &cap)
        .await
        .unwrap();
    assert!(a.load(&other, false).await.unwrap().is_none());
    assert!(b.load(&newest, false).await.unwrap().is_some());
    assert!(db
        .collection::<Document>(schema::COLLECTION)
        .find_one(doc! {"_id":schema::QUOTA})
        .await
        .unwrap()
        .is_some());
    assert!(db
        .collection::<Document>(schema::COLLECTION)
        .insert_one(doc! {"_id":"invalid","kind":"session"})
        .await
        .is_err());
}
