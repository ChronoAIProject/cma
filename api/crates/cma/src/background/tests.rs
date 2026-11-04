use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{sync::Notify, time::Instant};

use super::*;

fn work(budget: &Arc<BackgroundBudget>, name: &'static str) -> BackgroundWork {
    BackgroundWork::new(budget.clone(), name, Duration::from_secs(5), Duration::ZERO)
}

async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

fn hold(
    work: BackgroundWork,
    release: Arc<Notify>,
) -> tokio::task::JoinHandle<Result<Option<()>, SandboxStoreError>> {
    tokio::spawn(async move {
        work.run(|| async {
            release.notified().await;
            Ok(())
        })
        .await
    })
}

#[tokio::test(start_paused = true)]
async fn distinct_workers_share_one_budget_and_deferred_work_never_starts() {
    let budget = BackgroundBudget::new();
    let release = Arc::new(Notify::new());
    let agent = hold(work(&budget, "agent"), release.clone());
    let sandbox = hold(work(&budget, "sandbox"), release.clone());
    settle().await;
    let calls = AtomicUsize::new(0);
    let backfill = work(&budget, "backfill");
    assert_eq!(
        backfill
            .run(|| {
                calls.fetch_add(1, Ordering::SeqCst);
                async { Ok(()) }
            })
            .await
            .unwrap(),
        None
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(budget.slots.available_permits(), 0);
    release.notify_waiters();
    agent.await.unwrap().unwrap();
    sandbox.await.unwrap().unwrap();
    assert_eq!(backfill.run(|| async { Ok(7) }).await.unwrap(), Some(7));
    assert_eq!(budget.slots.available_permits(), 2);
}

#[tokio::test(start_paused = true)]
async fn foreground_bypasses_a_full_budget_and_leaves_background_progress() {
    let budget = BackgroundBudget::new();
    let release = Arc::new(Notify::new());
    let first = hold(work(&budget, "first"), release.clone());
    let second = hold(work(&budget, "second"), release.clone());
    settle().await;
    // This guard never awaits the background work, even when both slots are full.
    let foreground = budget.foreground();
    release.notify_waiters();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let remaining = hold(work(&budget, "recovery"), release.clone());
    settle().await;
    let optional = work(&budget, "optional");
    assert_eq!(optional.run(|| async { Ok(()) }).await.unwrap(), None);
    drop(foreground);
    assert_eq!(optional.run(|| async { Ok(()) }).await.unwrap(), Some(()));
    release.notify_waiters();
    remaining.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancellation_releases_the_permit_and_retains_retry_backoff() {
    let budget = BackgroundBudget::new();
    let work = work(&budget, "cancelled");
    let blocked = hold(work.clone(), Arc::new(Notify::new()));
    settle().await;
    assert_eq!(budget.slots.available_permits(), 1);
    blocked.abort();
    assert!(blocked.await.unwrap_err().is_cancelled());
    assert_eq!(budget.slots.available_permits(), 2);
    let started = Instant::now();
    assert_eq!(work.run(|| async { Ok(()) }).await.unwrap(), Some(()));
    assert!((Duration::from_millis(1500)..=Duration::from_secs(2)).contains(&started.elapsed()));
}

#[tokio::test(start_paused = true)]
async fn timeout_drops_the_operation_and_backoff_grows_then_resets_on_success() {
    struct Active(Arc<AtomicUsize>);
    impl Drop for Active {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let budget = BackgroundBudget::new();
    let work = work(&budget, "timeout");
    let active = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    assert!(work
        .run(|| async {
            active.fetch_add(1, Ordering::SeqCst);
            let _guard = Active(active.clone());
            std::future::pending::<Result<(), SandboxStoreError>>().await
        })
        .await
        .is_err());
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(budget.slots.available_permits(), 2);
    let mut waits = Vec::new();
    for _ in 0..8 {
        let started = Instant::now();
        assert!(work
            .run(|| async { Err::<(), _>(SandboxStoreError::Unavailable("fixture".into())) })
            .await
            .is_err());
        waits.push(started.elapsed());
    }
    assert!((Duration::from_millis(1500)..=Duration::from_secs(2)).contains(&waits[0]));
    assert!((Duration::from_secs(3)..=Duration::from_secs(4)).contains(&waits[1]));
    assert!((Duration::from_secs(45)..=Duration::from_secs(60)).contains(&waits[7]));
    work.run(|| async { Ok(()) }).await.unwrap();
    let started = Instant::now();
    work.run(|| async { Ok(()) }).await.unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_single_worker_never_multiplies_inflight_attempts() {
    let budget = BackgroundBudget::new();
    let work = work(&budget, "single");
    let release = Arc::new(Notify::new());
    let running = hold(work.clone(), release.clone());
    settle().await;
    let duplicate_calls = AtomicUsize::new(0);
    for _ in 0..100 {
        assert_eq!(
            work.run(|| async {
                duplicate_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap(),
            None
        );
    }
    assert_eq!(duplicate_calls.load(Ordering::SeqCst), 0);
    assert_eq!(budget.slots.available_permits(), 1);
    release.notify_waiters();
    running.await.unwrap().unwrap();
}
