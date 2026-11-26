use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI replica set"]
async fn lifecycle_burst_one_is_shared_and_stop_delete_refill_independently() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let database = client.database(&format!(
        "cma_control_limits_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut left = MongoRequestLimits::open(&database).await.unwrap();
    let peer = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let mut right = MongoRequestLimits::open(&peer.database(database.name()))
        .await
        .unwrap();
    left.set_test_clock_millis(1_000_000);
    right.set_test_clock_millis(1_000_000);
    for (key, rate, wait) in [("caller:stop", 6, 10), ("caller:delete", 3, 20)] {
        let outcomes = futures_util::future::join_all((0..8).map(|index| {
            let store = if index % 2 == 0 { &left } else { &right };
            async move { store.charge_with_burst(key, rate, 1, 1).await.unwrap() }
        }))
        .await;
        assert_eq!(outcomes.iter().filter(|result| result.is_none()).count(), 1);
        assert!(outcomes.iter().flatten().all(|delay| *delay == wait));
    }
    assert_eq!(
        right
            .charge_with_burst("other:stop", 6, 1, 1)
            .await
            .unwrap(),
        None
    );
    right.set_test_clock_millis(1_010_000);
    assert_eq!(
        right
            .charge_with_burst("caller:stop", 6, 1, 1)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        right
            .charge_with_burst("caller:delete", 3, 1, 1)
            .await
            .unwrap(),
        Some(10)
    );
    let restarted_client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let mut restarted = MongoRequestLimits::open(&restarted_client.database(database.name()))
        .await
        .unwrap();
    restarted.set_test_clock_millis(1_010_000);
    assert_eq!(
        restarted
            .charge_with_burst("caller:stop", 6, 1, 1)
            .await
            .unwrap(),
        Some(10)
    );
    restarted.set_test_clock_millis(1_020_000);
    assert_eq!(
        restarted
            .charge_with_burst("caller:delete", 3, 1, 1)
            .await
            .unwrap(),
        None
    );
    for (rate, burst, tokens) in [(0, 1, 1), (6, 0, 1), (6, 1, 0), (6, 1, 2)] {
        assert!(matches!(
            restarted
                .charge_with_burst("invalid", rate, burst, tokens)
                .await,
            Err(StoreError::Malformed(_))
        ));
    }
    assert_eq!(
        restarted
            .limits
            .count_documents(doc! {"_id":"invalid"})
            .await
            .unwrap(),
        0
    );
    database.drop().await.unwrap();
}
