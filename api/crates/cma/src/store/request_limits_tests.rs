#[path = "request_limits_tests/clock.rs"]
mod clock;
#[path = "request_limits_tests/contracts.rs"]
mod contracts;
#[path = "request_limits_tests/fixture.rs"]
mod fixture;
#[path = "request_limits_tests/legacy.rs"]
mod legacy;
#[path = "request_limits_tests/lifecycle.rs"]
mod lifecycle;
#[path = "request_limits_tests/ttl.rs"]
mod ttl;

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn independent_replicas_share_one_budget_and_restart_retains_it() {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!(
        "cma_request_limits_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let left = std::sync::Arc::new(MongoRequestLimits::open(&database).await.unwrap());
    let right = std::sync::Arc::new(MongoRequestLimits::open(&database).await.unwrap());
    let attempts = (0..8).map(|index| {
        let store = if index % 2 == 0 {
            left.clone()
        } else {
            right.clone()
        };
        async move { store.check("owner:create", 1).await.unwrap() }
    });
    let outcomes = futures_util::future::join_all(attempts).await;
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_none()).count(),
        1
    );
    assert!(outcomes
        .iter()
        .flatten()
        .all(|delay| (1..=60).contains(delay)));
    let restarted = MongoRequestLimits::open(&database).await.unwrap();
    assert!(restarted.check("owner:create", 1).await.unwrap().is_some());
    assert_eq!(restarted.check("other:create", 1).await.unwrap(), None);
    assert_eq!(restarted.check("owner:input", 1).await.unwrap(), None);
    let burst = (0..8).map(|index| {
        let store = if index % 2 == 0 {
            left.clone()
        } else {
            right.clone()
        };
        async move { store.check("owner:burst", 8).await.unwrap() }
    });
    assert!(futures_util::future::join_all(burst)
        .await
        .iter()
        .all(Option::is_none));
    assert!(matches!(
        restarted.check("owner:burst", 8).await.unwrap(),
        Some(1..=8)
    ));
    // An exhausted bucket refills according to stored time, independently of TTL cleanup.
    restarted
        .limits
        .update_one(
            doc! { "_id": "owner:create" },
            doc! { "$set": { "tat": 0_i64 } },
        )
        .await
        .unwrap();
    // A bucket can refill or be inserted between a failed admission and the
    // retry-delay lookup. Exercise Mongo's Int32 $max lower bound explicitly.
    assert_eq!(
        legacy::LegacyLimits {
            limits: restarted.limits.clone()
        }
        .retry_after("owner:create", 0)
        .await
        .unwrap(),
        Some(0)
    );
    assert_eq!(restarted.check("owner:create", 1).await.unwrap(), None);
    database.drop().await.unwrap();
}
