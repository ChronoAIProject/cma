use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn drain_closes_admission_joins_workers_and_is_repeatable() {
    let scope = TaskScope::new(2);
    let dropped = Arc::new(AtomicUsize::new(0));
    for _ in 0..2 {
        let dropped = Dropped(dropped.clone());
        assert!(scope.spawn(async move {
            let _dropped = dropped;
            std::future::pending::<()>().await;
        }));
    }
    assert!(!scope.spawn(async {}));
    scope.drain(Duration::from_secs(1)).await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
    assert!(scope.is_closed());
    assert!(!scope.spawn(async { panic!("closed admission ran work") }));
    scope.clone().drain(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn shutdown_registration_closes_discovery_and_joins_its_captured_workers() {
    let scope = TaskScope::new(2);
    let dropped = Arc::new(AtomicUsize::new(0));
    let worker = Dropped(dropped.clone());
    assert!(scope.spawn(async move {
        let _worker = worker;
        std::future::pending::<()>().await;
    }));
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.register_lifecycle(scope.clone());
    shutdown.signal();
    assert!(scope.is_closed());
    assert!(!scope.spawn(async { panic!("late discovery started work") }));
    shutdown
        .drain_lifecycles(Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert_eq!(shutdown.drain().await, 0);
}

#[tokio::test]
async fn two_hundred_callers_share_one_bounded_task_set() {
    let scope = TaskScope::new(17);
    let mut callers = JoinSet::new();
    for _ in 0..200 {
        let scope = scope.clone();
        callers.spawn(async move { scope.spawn(std::future::pending()) });
    }
    let mut admitted = 0;
    while let Some(result) = callers.join_next().await {
        admitted += usize::from(result.unwrap());
    }
    assert_eq!(admitted, 17);
    scope.drain(Duration::from_secs(1)).await.unwrap();
    assert!(!scope.spawn(std::future::pending()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_drain_retains_joins_until_uncooperative_worker_exits() {
    let scope = TaskScope::new(1);
    let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let worker_release = release.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    assert!(scope.spawn(async move {
        entered.send(()).unwrap();
        let (lock, wake) = &*worker_release;
        let guard = lock.lock().unwrap();
        drop(wake.wait_while(guard, |released| !*released).unwrap());
    }));
    ready.await.unwrap();
    let timed_out = scope.drain(Duration::from_millis(10)).await.is_err();
    let still_closed = !scope.spawn(async {});
    let (lock, wake) = &*release;
    *lock.lock().unwrap() = true;
    wake.notify_all();
    assert!(timed_out);
    assert!(still_closed);
    scope.drain(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn completed_and_panicking_tasks_release_capacity() {
    let scope = TaskScope::new(1);
    assert!(scope.spawn(async { panic!("fixture task failure") }));
    tokio::task::yield_now().await;
    assert!(scope.spawn(async {}));
    scope.drain(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn completing_drain_joins_work_after_closing_admission() {
    let scope = TaskScope::completing(1);
    let (release, waiting) = tokio::sync::oneshot::channel();
    let (acknowledge, completed) = tokio::sync::oneshot::channel();
    assert!(scope.spawn(async move {
        waiting.await.unwrap();
        acknowledge.send(()).unwrap();
    }));

    scope.close();
    assert!(!scope.spawn(async {}));
    release.send(()).unwrap();
    scope.drain(Duration::from_secs(1)).await.unwrap();
    completed.await.unwrap();
}
