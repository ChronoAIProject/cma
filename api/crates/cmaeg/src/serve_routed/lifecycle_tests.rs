use super::*;
use crate::{
    ingress::routing_store::{AdmissionError, RecoveryPage},
    turns::routed::scheduler::{
        Disposition, RecoverySource, ScheduleConfig, TargetExecutor, WorkItem,
    },
};
use futures_util::future::BoxFuture;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

struct Source {
    started: Notify,
    dropped: Arc<AtomicBool>,
    panic: bool,
}
struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl RecoverySource for Source {
    fn page<'a>(
        &'a self,
        _: Option<&'a str>,
    ) -> BoxFuture<'a, Result<RecoveryPage, AdmissionError>> {
        Box::pin(async move {
            let _dropped = Dropped(self.dropped.clone());
            self.started.notify_one();
            assert!(!self.panic, "injected scanner failure");
            std::future::pending().await
        })
    }
}
struct Executor;
impl TargetExecutor for Executor {
    fn execute<'a>(&'a self, _: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        unreachable!("empty source never dispatches work")
    }
}
async fn setup(panic: bool) -> (Worker, Readiness, Arc<Source>) {
    let source = Arc::new(Source {
        started: Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
        panic,
    });
    let (scheduler, _) = Scheduler::new(
        source.clone(),
        Arc::new(Executor),
        ScheduleConfig::default(),
    )
    .unwrap();
    let readiness = Readiness::initialized();
    let worker = Worker::start(scheduler, readiness.clone());
    tokio::time::timeout(Duration::from_secs(1), source.started.notified())
        .await
        .unwrap();
    (worker, readiness, source)
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn recovery_failure_removes_readiness_and_is_not_reported_as_a_clean_drain() {
    let (mut worker, readiness, source) = setup(true).await;
    until(|| !readiness.report().ready).await;
    assert_eq!(readiness.report().reasons, ["routed_recovery_failed"]);
    assert!(!worker.drain(Instant::now() + Duration::from_secs(1)).await);
    assert!(source.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn clean_shutdown_stops_pending_scan_without_marking_worker_failed() {
    let (mut worker, readiness, source) = setup(false).await;
    assert!(worker.drain(Instant::now() + Duration::from_secs(1)).await);
    assert!(readiness.report().ready);
    assert!(source.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn dropping_service_cancels_its_scanner_instead_of_detaching_it() {
    let (worker, _, source) = setup(false).await;
    drop(worker);
    until(|| source.dropped.load(Ordering::SeqCst)).await;
}
