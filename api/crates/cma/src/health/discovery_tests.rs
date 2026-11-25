use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

use crate::worker_supervision::{self, Activity, Workers};

struct DelayedProbe(AtomicUsize);

#[async_trait::async_trait]
impl ReadinessProbe for DelayedProbe {
    async fn ping(&self) -> Result<(), StoreError> {
        tokio::time::sleep(Duration::from_millis(self.0.load(Ordering::SeqCst) as u64)).await;
        Ok(())
    }
}

async fn advance() {
    tokio::time::advance(Duration::from_millis(100)).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

async fn schedule(
    children: &mut JoinSet<Result<(), &'static str>>,
    activity: Activity,
    shutdown: watch::Receiver<bool>,
    timeouts: Arc<AtomicUsize>,
) {
    loop {
        activity.waiting(Duration::from_millis(250), "discovery");
        tokio::select! {
            _ = crate::shutdown::signalled(shutdown.clone()) => return,
            result = children.join_next(), if !children.is_empty() => {
                assert_eq!(result.unwrap().unwrap(), Err("discovery deadline"));
                timeouts.fetch_add(1, Ordering::SeqCst);
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        }
        activity.scheduled();
    }
}

#[tokio::test(start_paused = true)]
async fn slow_store_checks_and_expired_discovery_do_not_flap_worker_readiness() {
    verify_slow_dependencies(None).await;
}

#[tokio::test(start_paused = true)]
async fn diagnostic_capture_preserves_readiness_during_slow_dependencies() {
    use crate::readiness_diagnostics::{fixture::Capture, record::Record};

    let capture = Capture::new();
    verify_slow_dependencies(Some(capture.recorder.clone())).await;
    let records = capture.records();
    assert!(records.iter().any(|record| matches!(
        record,
        Record::Response {
            reason: "deadline",
            ..
        }
    )));
    assert!(!records.iter().any(|record| matches!(
        record,
        Record::Response {
            reason: "workers",
            ..
        }
    )));
}

async fn verify_slow_dependencies(diagnostics: Option<Arc<Recorder>>) {
    let workers = Workers::new();
    // Other capabilities have already completed a turn and are in valid waits.
    for worker in workers.snapshot().iter().filter(|worker| worker.required) {
        let activity = workers.get(worker.worker);
        activity.scheduled();
        activity.waiting(Duration::from_secs(60), "idle_scan");
    }
    let activity = workers.get("agent_commands");
    let mut children = JoinSet::new();
    children.spawn(worker_supervision::discover(
        Some(activity.clone()),
        std::future::pending::<Result<(), &'static str>>(),
        "discovery deadline",
    ));
    let starts = Arc::new(AtomicUsize::new(0));
    let timeouts = Arc::new(AtomicUsize::new(0));
    let (stop, signal) = watch::channel(false);
    let attempts = starts.clone();
    let expired = timeouts.clone();
    let task = worker_supervision::spawn(
        children,
        activity,
        signal,
        move |children, activity, shutdown| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Box::pin(schedule(children, activity, shutdown, expired.clone()))
        },
        |children| Box::pin(children.shutdown()),
    );
    // A successful ping just inside the unchanged deadline leaves little slack.
    let probe = Arc::new(DelayedProbe(AtomicUsize::new(1900)));
    let app = monitored(
        probe.clone(),
        Some(workers.clone()),
        false,
        diagnostics,
        None,
    );
    for _ in 0..21 {
        advance().await;
    }
    for _ in 0..100 {
        let (status, body) = tests::response(&app, "/ready").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["dependency"]["duration_ms"], 1900);
        assert!(workers.snapshot().iter().all(|worker| worker.available()));
        advance().await;
    }
    assert_eq!(timeouts.load(Ordering::SeqCst), 1);
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    // A true dependency deadline must still fail closed without blaming workers.
    probe.0.store(2100, Ordering::SeqCst);
    for _ in 0..60 {
        advance().await;
    }
    let (status, body) = tests::response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason"], "store");
    assert_eq!(body["dependency"]["reason"], "deadline");
    assert_eq!(body["dependency"]["duration_ms"], 2000);
    assert!(workers.snapshot().iter().all(|worker| worker.available()));
    assert_eq!(tests::response(&app, "/livez").await.0, StatusCode::OK);

    probe.0.store(1900, Ordering::SeqCst);
    for _ in 0..60 {
        advance().await;
    }
    assert_eq!(tests::response(&app, "/ready").await.0, StatusCode::OK);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    task.await.unwrap();
}
