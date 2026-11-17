use super::super::*;
use super::{fixture::Fixture, legacy::LegacyLimits};
use mongodb::bson::Bson;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn fresh_and_warm_concurrency_across_independent_clients_spends_exact_tokens() {
    let f = Fixture::new().await;
    for (key, warm) in [("fresh", 0), ("warm", 3)] {
        for _ in 0..warm {
            assert_eq!(f.left.check(key, 8).await.unwrap(), None);
        }
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(65));
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..64 {
            let (store, barrier) = (
                if index % 2 == 0 {
                    f.left.clone()
                } else {
                    f.right.clone()
                },
                barrier.clone(),
            );
            tasks.spawn(async move {
                barrier.wait().await;
                store.check(key, 8).await.unwrap()
            });
        }
        let start = std::time::Instant::now();
        barrier.wait().await;
        let mut admitted = 0;
        while let Some(result) = tasks.join_next().await {
            admitted += usize::from(result.unwrap().is_none());
        }
        assert!(
            start.elapsed() < Duration::from_millis(7500),
            "cohort must finish before the first token refill"
        );
        assert_eq!(admitted, 8 - warm, "{key}");
    }
    f.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn duplicate_insert_race_on_one_fresh_key_retries_without_extra_admission() {
    let f = Fixture::new().await;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let left = f.left.clone();
    let right = f.right.clone();
    let a = {
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            left.check("race", 1).await
        })
    };
    let b = {
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            right.check("race", 1).await
        })
    };
    barrier.wait().await;
    let outcomes = [a.await.unwrap().unwrap(), b.await.unwrap().unwrap()];
    assert_eq!(outcomes.iter().filter(|value| value.is_none()).count(), 1);
    assert!(outcomes.iter().flatten().all(|delay| *delay >= 1));
    let commands = f.take();
    assert!(
        commands
            .iter()
            .filter(|command| command.contains_key("findAndModify"))
            .count()
            >= 2
    );
    assert!(f.left.check("race", 1).await.unwrap().is_some());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set with failpoints"]
async fn injected_duplicate_key_on_first_upsert_retries_without_extra_admission() {
    let f = Fixture::new().await;
    f.fail(11_000, 1).await;
    assert_eq!(f.left.check("injected-race", 1).await.unwrap(), None);
    f.clear_failpoint().await;

    let commands = f.take();
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.contains_key("findAndModify"))
            .count(),
        2,
        "the duplicate-key response must be retried once"
    );
    assert!(f.left.check("injected-race", 1).await.unwrap().is_some());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set with failpoints"]
async fn database_errors_never_create_process_local_allowance() {
    let f = Fixture::new().await;
    for store in [&f.left, &f.right] {
        f.fail(13, 1).await;
        let result = store.check("unavailable", 1).await;
        f.clear_failpoint().await;
        assert!(matches!(result, Err(StoreError::Unavailable(_))));
    }
    assert!(f
        .left
        .limits
        .find_one(doc! {"_id":"unavailable"})
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.left.check("unavailable", 1).await.unwrap(), None);
    assert!(f.right.check("unavailable", 1).await.unwrap().is_some());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn legacy_rows_and_rollback_upgrade_preserve_shared_budget() {
    let f = Fixture::new().await;
    let old = LegacyLimits::open(&f.database).await.unwrap();
    f.seed("legacy", Bson::Int32(0)).await;
    assert_eq!(f.left.charge("legacy", 4, 2).await.unwrap(), None);
    assert_eq!(old.check("legacy", 4).await.unwrap(), None);
    assert_eq!(f.right.check("legacy", 4).await.unwrap(), None);
    assert!(old.check("legacy", 4).await.unwrap().is_some());
    assert!(f.left.check("legacy", 4).await.unwrap().is_some());
    let row = f
        .left
        .limits
        .find_one(doc! {"_id":"legacy"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i32("document_schema_version").unwrap(), 1);
    assert!(row.get_i64("tat").unwrap() > 0);
    assert_eq!(row.len(), 4, "no persisted decision or clock sidecar");
    f.close().await;
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn complete_command_counts_compare_frozen_legacy_and_upsert() {
    let f = Fixture::new().await;
    let old = LegacyLimits::open(&f.database).await.unwrap();
    f.take();
    assert_eq!(old.check("old", 2).await.unwrap(), None);
    let old_cold = f.take();
    assert_eq!(old_cold.len(), 4);
    assert_eq!(old.check("old", 2).await.unwrap(), None);
    assert_eq!(f.take().len(), 1);
    assert!(old.check("old", 2).await.unwrap().is_some());
    assert_eq!(f.take().len(), 2);
    assert_eq!(f.left.check("new", 2).await.unwrap(), None);
    let new_cold = f.take();
    assert_eq!(new_cold.len(), 1);
    assert_eq!(f.left.check("new", 2).await.unwrap(), None);
    assert_eq!(f.take().len(), 1);
    assert!(f.left.check("new", 2).await.unwrap().is_some());
    assert_eq!(f.take().len(), 1);
    println!("REQUEST_LIMIT_COMMANDS cold=4->1 warm_admit=1->1 warm_deny=2->1; every command-start counted, including any getMore/auth/retry");
    f.close().await;
}
