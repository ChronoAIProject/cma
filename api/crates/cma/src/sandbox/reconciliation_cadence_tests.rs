use super::*;
use crate::background::{BackgroundBudget, BackgroundWork};
use std::sync::Arc;
use tokio::sync::{watch, Notify};

async fn settle() {
    for _ in 0..40 {
        tokio::task::yield_now().await;
    }
}

fn work(budget: Arc<BackgroundBudget>) -> BackgroundWork {
    BackgroundWork::new(
        budget,
        "sandbox_reconciliation",
        Duration::from_secs(30),
        Duration::from_secs(15),
    )
}

fn spawn(reconciler: SandboxReconciler) -> (watch::Sender<bool>, tokio::task::JoinHandle<()>) {
    let (stop, signal) = watch::channel(false);
    let activity = crate::worker_supervision::Workers::new().get("sandbox_reconciliation");
    (stop, reconciler.spawn_supervised(signal, activity))
}

#[tokio::test(start_paused = true)]
async fn budget_denial_resumes_at_work_deadline_while_disabled_mode_keeps_its_tick() {
    for enabled in [false, true] {
        let shared = BackgroundBudget::new();
        let _foreground = shared.foreground();
        let optional = BackgroundWork::new(
            shared.clone(),
            "fixture_optional",
            Duration::from_secs(60),
            Duration::ZERO,
        );
        let release = Arc::new(Notify::new());
        let released = release.clone();
        let occupied = tokio::spawn(async move {
            optional
                .run(|| async {
                    released.notified().await;
                    Ok(())
                })
                .await
        });
        settle().await;
        let (store, reconciler) = setup(1, Duration::ZERO).await;
        let work = work(shared);
        let (stop, scheduler) = spawn(
            reconciler
                .with_background_work(work.clone())
                .with_background_resumption(enabled),
        );
        settle().await;
        assert!(store.keys("authority").is_empty());
        let retry = work.next_run_at();
        assert!((Duration::from_millis(187)..=Duration::from_millis(250))
            .contains(&retry.duration_since(Instant::now())));
        release.notify_one();
        occupied.await.unwrap().unwrap();
        tokio::time::advance(retry.duration_since(Instant::now())).await;
        settle().await;
        if enabled {
            assert_eq!(store.keys("authority"), ["start"]);
        } else {
            assert!(store.keys("authority").is_empty());
            tokio::time::advance(Duration::from_secs(15)).await;
            settle().await;
            assert_eq!(store.keys("authority"), ["start"]);
        }
        stop.send(true).unwrap();
        scheduler.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn failures_keep_backoff_and_completion_keeps_the_jittered_fifteen_second_interval() {
    let (store, reconciler) = setup(1, Duration::ZERO).await;
    store.fail("authority", "start", Failure::Unavailable);
    let work = work(BackgroundBudget::new());
    let (stop, scheduler) = spawn(
        reconciler
            .with_background_work(work.clone())
            .with_background_resumption(true),
    );
    settle().await;
    assert_eq!(store.keys("authority"), ["start"]);
    let retry = work.next_run_at();
    assert!((Duration::from_millis(1500)..=Duration::from_secs(2))
        .contains(&retry.duration_since(Instant::now())));
    tokio::time::advance(retry.duration_since(Instant::now()) - Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(store.keys("authority").len(), 1);
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(store.keys("authority").len(), 2);
    let completed = work.next_run_at();
    assert!((Duration::from_millis(11250)..=Duration::from_secs(15))
        .contains(&completed.duration_since(Instant::now())));
    tokio::time::advance(completed.duration_since(Instant::now()) - Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(store.keys("authority").len(), 2);
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(store.keys("authority").len(), 3);
    let stopped = Instant::now();
    stop.send(true).unwrap();
    scheduler.await.unwrap();
    assert_eq!(Instant::now(), stopped, "shutdown interrupts the idle wait");
}

#[tokio::test(start_paused = true)]
async fn yielded_sweep_resumes_without_an_extra_fifteen_second_tick() {
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    let work = work(BackgroundBudget::new());
    let (stop, scheduler) = spawn(
        reconciler
            .with_background_work(work.clone())
            .with_background_resumption(true),
    );
    settle().await;
    for _ in 0..30 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        if work.next_run_at() > Instant::now() {
            break;
        }
    }
    let retry = work.next_run_at();
    assert!((Duration::from_millis(187)..=Duration::from_millis(250))
        .contains(&retry.saturating_duration_since(Instant::now())));
    let before = store.keys("get").len();
    assert!(before > 0 && before < 8);
    tokio::time::advance(retry.duration_since(Instant::now())).await;
    settle().await;
    assert!(store.keys("get").len() > before);
    stop.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reserved_driver_keeps_one_owned_attempt_and_drains_it_on_shutdown() {
    let (store, reconciler) = setup(8, Duration::from_secs(2)).await;
    let shared = BackgroundBudget::new();
    let work = work(shared.reserve(1).unwrap());
    let (stop, scheduler) = spawn(
        reconciler
            .with_background_work(work)
            .with_background_resumption(true),
    );
    settle().await;
    for _ in 0..7 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(
            store
                .keys("authority")
                .iter()
                .filter(|key| *key == "start")
                .count(),
            1
        );
    }
    let stopping = Instant::now();
    stop.send(true).unwrap();
    settle().await;
    assert!(!scheduler.is_finished());
    scheduler.await.unwrap();
    assert!(Instant::now() > stopping);
    assert_eq!(
        store.keys("authority"),
        ["start", "sbx_001", "sbx_003", "sbx_005"]
    );
}
