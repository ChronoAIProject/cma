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
    // An exhausted bucket refills according to stored time, independently of TTL cleanup.
    restarted
        .limits
        .update_one(
            doc! { "_id": "owner:create" },
            doc! { "$set": { "tat": 0_i64 } },
        )
        .await
        .unwrap();
    assert_eq!(restarted.check("owner:create", 1).await.unwrap(), None);
    database.drop().await.unwrap();
}
