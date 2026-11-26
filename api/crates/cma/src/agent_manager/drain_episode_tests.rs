use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Notify;

#[tokio::test(start_paused = true)]
async fn concurrent_and_repeated_callers_keep_one_original_deadline_and_first_outcome() {
    let episode = Arc::new(DrainEpisode::new(
        DrainCause::PreStop,
        Instant::now() + Duration::from_secs(63),
    ));
    let entered = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let first = tokio::spawn({
        let episode = episode.clone();
        let entered = entered.clone();
        let calls = calls.clone();
        async move {
            episode
                .run(async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    std::future::pending().await
                })
                .await
        }
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(20)).await;
    let next = tokio::spawn({
        let episode = episode.clone();
        let calls = calls.clone();
        async move {
            episode
                .run(async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        }
    });
    assert!(first.await.unwrap().is_err());
    assert!(next.await.unwrap().is_err());
    assert_eq!(episode.started.elapsed(), Duration::from_secs(23));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(episode.cause, DrainCause::PreStop);
    assert!(episode
        .run(async { panic!("terminal drain cannot restart") })
        .await
        .is_err());
}

#[tokio::test(start_paused = true)]
async fn cancelled_driver_can_resume_only_within_original_remaining_budget() {
    let episode = Arc::new(DrainEpisode::new(
        DrainCause::Shutdown,
        Instant::now() + Duration::from_secs(63),
    ));
    let entered = Arc::new(Notify::new());
    let first = tokio::spawn({
        let episode = episode.clone();
        let entered = entered.clone();
        async move {
            episode
                .run(async {
                    entered.notify_one();
                    std::future::pending().await
                })
                .await
        }
    });
    entered.notified().await;
    tokio::time::advance(Duration::from_secs(21)).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let result = episode
        .run(async {
            tokio::time::sleep(Duration::from_secs(3)).await;
            Ok(())
        })
        .await;
    assert!(result.is_err());
    assert_eq!(episode.started.elapsed(), Duration::from_secs(23));
}

#[tokio::test(start_paused = true)]
async fn failed_drain_cannot_become_success_and_lock_wait_spends_the_budget() {
    let failed = DrainEpisode::new(
        DrainCause::OwnershipFailure,
        Instant::now() + Duration::from_secs(63),
    );
    let error = "synthetic unconfirmed custody".to_string();
    assert_eq!(
        failed.run(async { Err(error.clone()) }).await,
        Err(error.clone())
    );
    assert_eq!(failed.run(async { Ok(()) }).await, Err(error));
    let blocked = DrainEpisode::new(
        DrainCause::HttpFailure,
        Instant::now() + Duration::from_secs(4),
    );
    let _lock = blocked.driver.lock().await;
    assert!(blocked
        .run(async { panic!("lock wait exhausted the budget") })
        .await
        .is_err());
    assert_eq!(blocked.started.elapsed(), Duration::from_secs(4));
}

#[tokio::test(start_paused = true)]
async fn completed_drain_is_retained_without_executing_another_driver() {
    let episode = DrainEpisode::new(
        DrainCause::Shutdown,
        Instant::now() + Duration::from_secs(63),
    );
    episode.run(async { Ok(()) }).await.unwrap();
    tokio::time::advance(Duration::from_secs(70)).await;
    episode
        .run(async { panic!("success is already retained") })
        .await
        .unwrap();
}
