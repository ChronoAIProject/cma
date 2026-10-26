use super::{scheduler::*, test_support::Setup, RecoverySupport};
use crate::ingress::routing_store::{AdmissionError, AdmittedPlan, RecoveryPage};
use futures_util::future::BoxFuture;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch, Semaphore};

struct Executor {
    started: mpsc::UnboundedSender<String>,
    permits: Semaphore,
    calls: AtomicUsize,
    panic_first: bool,
}
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.send(work.target_id.clone()).unwrap();
            assert!(
                !(self.panic_first && call == 0),
                "injected executor failure"
            );
            self.permits.acquire().await.unwrap().forget();
            Disposition::Settled
        })
    }
}
type PageResult = Result<RecoveryPage, AdmissionError>;
struct Pages {
    values: Mutex<VecDeque<PageResult>>,
    cursors: Mutex<Vec<Option<String>>>,
    repeated: Vec<AdmittedPlan>,
    panic: bool,
}
impl RecoverySource for Pages {
    fn page<'a>(&'a self, after: Option<&'a str>) -> BoxFuture<'a, PageResult> {
        Box::pin(async move {
            assert!(!self.panic, "injected recovery failure");
            self.cursors.lock().unwrap().push(after.map(str::to_owned));
            self.values
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(page(self.repeated.clone(), None)))
        })
    }
}
fn page(plans: Vec<AdmittedPlan>, cursor: Option<&str>) -> RecoveryPage {
    RecoveryPage {
        plans,
        issues: Vec::new(),
        next_cursor: cursor.map(str::to_owned),
    }
}
fn config() -> ScheduleConfig {
    ScheduleConfig {
        concurrency: 1,
        queue_capacity: 8,
        page_delay: Duration::from_millis(10),
        sweep_delay: Duration::from_millis(10),
        drain_timeout: Duration::from_millis(100),
    }
}
async fn setup() -> Setup {
    Setup::new(RecoverySupport {
        exact_body_idempotency: false,
        response_replay: true,
    })
    .await
}
fn make_executor(
    permits: usize,
    panic_first: bool,
) -> (Arc<Executor>, mpsc::UnboundedReceiver<String>) {
    let (started, received) = mpsc::unbounded_channel();
    (
        Arc::new(Executor {
            started,
            permits: Semaphore::new(permits),
            calls: AtomicUsize::new(0),
            panic_first,
        }),
        received,
    )
}
async fn next(receiver: &mut mpsc::UnboundedReceiver<String>) -> String {
    tokio::time::timeout(Duration::from_secs(3), receiver.recv())
        .await
        .unwrap()
        .unwrap()
}
fn empty_source() -> Arc<Pages> {
    Arc::new(Pages {
        values: Mutex::new(VecDeque::new()),
        cursors: Mutex::new(Vec::new()),
        repeated: Vec::new(),
        panic: false,
    })
}

#[tokio::test]
async fn a_full_hint_queue_recovers_every_committed_target_from_the_real_store() {
    let setup = setup().await;
    assert!(setup.plan.targets.len() > 1);
    let (executor, mut started) = make_executor(10, false);
    let (scheduler, handle) = Scheduler::new(
        Arc::new(setup.server.store()),
        executor.clone(),
        ScheduleConfig {
            queue_capacity: 1,
            ..config()
        },
    )
    .unwrap();
    assert!(!handle.notify(&setup.plan));
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    let mut observed = std::collections::BTreeSet::new();
    while observed.len() < setup.plan.targets.len() {
        observed.insert(next(&mut started).await);
    }
    stop.send(true).unwrap();
    let report = task.await.unwrap();
    assert_eq!(
        observed,
        setup
            .plan
            .targets
            .iter()
            .map(|target| target.id.clone())
            .collect()
    );
    assert_eq!(report.aborted, 0);
    assert!(!report.recovery_failed);
}

#[tokio::test]
async fn repeated_hints_are_deduplicated_and_shutdown_drains_only_running_work() {
    let setup = setup().await;
    let (executor, mut started) = make_executor(0, false);
    let (scheduler, handle) = Scheduler::new(empty_source(), executor.clone(), config()).unwrap();
    assert!(handle.notify(&setup.plan));
    assert!(handle.notify(&setup.plan));
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    next(&mut started).await;
    stop.send(true).unwrap();
    executor.permits.add_permits(10);
    let report = task.await.unwrap();
    assert_eq!(report.started, 1);
    assert_eq!(report.finished, 1);
    assert_eq!(report.aborted, 0);
    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn duplicates_do_not_run_concurrently_and_terminal_scan_results_cool_down() {
    let setup = setup().await;
    assert_eq!(setup.plan.targets.len(), 3);
    let (executor, mut started) = make_executor(0, false);
    let source = Arc::new(Pages {
        repeated: vec![setup.plan.clone()],
        ..Arc::try_unwrap(empty_source()).ok().unwrap()
    });
    let (scheduler, handle) = Scheduler::new(
        source,
        executor.clone(),
        ScheduleConfig {
            concurrency: 2,
            ..config()
        },
    )
    .unwrap();
    handle.notify(&setup.plan);
    handle.notify(&setup.plan);
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    let mut seen = std::collections::BTreeSet::new();
    seen.insert(next(&mut started).await);
    seen.insert(next(&mut started).await);
    assert_eq!(seen.len(), 2);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), started.recv())
            .await
            .is_err()
    );
    executor.permits.add_permits(1);
    seen.insert(next(&mut started).await);
    assert_eq!(seen.len(), 3);
    executor.permits.add_permits(2);
    assert!(
        tokio::time::timeout(Duration::from_millis(80), started.recv())
            .await
            .is_err()
    );
    stop.send(true).unwrap();
    let report = task.await.unwrap();
    assert_eq!(report.started as usize, setup.plan.targets.len());
    assert_eq!(report.aborted, 0);
}

#[tokio::test]
async fn recovery_crosses_empty_pages_retries_same_cursor_and_rescans_from_start() {
    let setup = setup().await;
    let source = Arc::new(Pages {
        values: Mutex::new(VecDeque::from([
            Ok(page(Vec::new(), Some("first-page"))),
            Err(AdmissionError::Unavailable),
            Ok(page(Vec::new(), None)),
            Ok(page(vec![setup.plan.clone()], None)),
        ])),
        ..Arc::try_unwrap(empty_source()).ok().unwrap()
    });
    let (executor, mut started) = make_executor(10, false);
    let (scheduler, _) = Scheduler::new(source.clone(), executor, config()).unwrap();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    for _ in &setup.plan.targets {
        next(&mut started).await;
    }
    stop.send(true).unwrap();
    assert!(!task.await.unwrap().recovery_failed);
    assert_eq!(
        &source.cursors.lock().unwrap()[..4],
        &[
            None,
            Some("first-page".into()),
            Some("first-page".into()),
            None
        ]
    );
}

#[tokio::test]
async fn executor_panics_release_local_deduplication_for_durable_recovery() {
    let mut setup = setup().await;
    setup.plan.targets.truncate(1);
    let source = Arc::new(Pages {
        repeated: vec![setup.plan.clone()],
        ..Arc::try_unwrap(empty_source()).ok().unwrap()
    });
    let (executor, mut started) = make_executor(10, true);
    let (scheduler, _) = Scheduler::new(source, executor, config()).unwrap();
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    assert_eq!(next(&mut started).await, next(&mut started).await);
    stop.send(true).unwrap();
    let report = task.await.unwrap();
    assert_eq!(report.started, 2);
    assert!(!report.recovery_failed);
}

#[tokio::test]
async fn shutdown_deadline_aborts_only_remaining_tasks_and_reports_it() {
    let setup = setup().await;
    let (executor, mut started) = make_executor(0, false);
    let (scheduler, handle) = Scheduler::new(empty_source(), executor, config()).unwrap();
    handle.notify(&setup.plan);
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(scheduler.run(stopped));
    next(&mut started).await;
    stop.send(true).unwrap();
    let report = task.await.unwrap();
    assert_eq!(report.aborted, 1);
    assert_eq!(report.started, 1);
}

#[tokio::test]
async fn false_watch_updates_do_not_shutdown_and_broken_recovery_is_visible() {
    let setup = setup().await;
    let (executor, mut started) = make_executor(10, false);
    let (scheduler, handle) = Scheduler::new(empty_source(), executor, config()).unwrap();
    let (stop, stopped) = watch::channel(false);
    stop.send(false).unwrap();
    let task = tokio::spawn(scheduler.run(stopped));
    handle.notify(&setup.plan);
    next(&mut started).await;
    stop.send(true).unwrap();
    assert!(!task.await.unwrap().recovery_failed);

    let source = Arc::new(Pages {
        panic: true,
        ..Arc::try_unwrap(empty_source()).ok().unwrap()
    });
    let (executor, _) = make_executor(10, false);
    let (scheduler, _) = Scheduler::new(source, executor, config()).unwrap();
    let (_stop, stopped) = watch::channel(false);
    let report = tokio::time::timeout(Duration::from_secs(1), scheduler.run(stopped))
        .await
        .unwrap();
    assert!(report.recovery_failed);
}

#[test]
fn scheduling_bounds_reject_unbounded_or_zero_capacity() {
    let (executor, _) = make_executor(0, false);
    for config in [
        ScheduleConfig {
            queue_capacity: 0,
            ..config()
        },
        ScheduleConfig {
            concurrency: 257,
            ..config()
        },
        ScheduleConfig {
            page_delay: Duration::ZERO,
            ..config()
        },
    ] {
        assert!(Scheduler::new(empty_source(), executor.clone(), config).is_err());
    }
}
