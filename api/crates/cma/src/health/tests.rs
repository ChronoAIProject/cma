use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use serde_json::Value;
use tower::ServiceExt as _;

use super::*;

// 0: success, 1: error, 2: never resolves (checkout or command).
#[derive(Default)]
struct Probe {
    mode: AtomicUsize,
    calls: AtomicUsize,
    active: Arc<AtomicUsize>,
}

struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ReadinessProbe for Probe {
    async fn ping(&self) -> Result<(), StoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = Active(self.active.clone());
        match self.mode.load(Ordering::SeqCst) {
            0 => Ok(()),
            1 => Err(StoreError::Unavailable("offline".into())),
            _ => std::future::pending().await,
        }
    }
}

pub(super) async fn response(app: &Router, path: &str) -> (StatusCode, Value) {
    let response = tokio::time::timeout(
        Duration::from_millis(100),
        app.clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap()),
    )
    .await
    .expect("health route must answer independently of the store")
    .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn tick(duration: Duration) {
    tokio::time::advance(duration).await;
    tokio::task::yield_now().await;
}

#[tokio::test(start_paused = true)]
async fn startup_and_unavailable_checks_fail_closed_and_recover() {
    let probe = Arc::new(Probe::default());
    probe.mode.store(1, Ordering::SeqCst);
    let app = router(probe.clone());
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"]["reason"], "pending");
    tokio::task::yield_now().await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"]["reason"], "unavailable");
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    probe.mode.store(0, Ordering::SeqCst);
    tick(CHECK_INTERVAL).await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
    assert!(body["dependency"]["checked_at"].is_string());
    assert!(body["dependency"]["reason"].is_null());
}

#[tokio::test(start_paused = true)]
async fn backend_free_readiness_depends_on_the_store_and_has_no_execution_workers() {
    let probe = Arc::new(Probe::default());
    probe.mode.store(1, Ordering::SeqCst);
    let app = backend_free_router(probe.clone());
    for reason in ["pending", "unavailable"] {
        let (status, body) = response(&app, "/ready").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["dependency"]["reason"], reason);
        assert_eq!(body["sandbox"]["available"], false);
        assert_eq!(body["workers"], serde_json::json!([]));
        tokio::task::yield_now().await;
    }
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    probe.mode.store(0, Ordering::SeqCst);
    tick(CHECK_INTERVAL).await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["sandbox"]["available"], false);
    assert_eq!(body["workers"], serde_json::json!([]));
}

#[tokio::test(start_paused = true)]
async fn unanswered_checks_are_single_flight_bounded_cancelled_and_recover() {
    let probe = Arc::new(Probe::default());
    probe.mode.store(2, Ordering::SeqCst);
    let app = router(probe.clone());
    tokio::task::yield_now().await;
    for _ in 0..100 {
        assert_eq!(
            response(&app, "/ready").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    tick(CHECK_DEADLINE).await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["dependency"]["reason"], "deadline");
    assert_eq!(body["dependency"]["duration_ms"], 2000);
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    probe.mode.store(0, Ordering::SeqCst);
    tick(CHECK_INTERVAL).await;
    assert_eq!(response(&app, "/ready").await.0, StatusCode::OK);
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn stale_success_fails_closed_even_if_the_monitor_stops() {
    let (_sender, state) = watch::channel(Some(CompletedCheck {
        completed: Instant::now(),
        checked_at: "2026-11-05T00:00:00Z".into(),
        duration_ms: 5,
        failure: None,
        diagnostic_check: 0,
    }));
    let apps = [
        routes(state.clone(), None),
        health_routes(HealthState {
            store: state,
            workers: None,
            diagnostics: None,
            backend_free: true,
            runtime_release: None,
        }),
    ];
    for app in &apps {
        assert_eq!(response(app, "/ready").await.0, StatusCode::OK);
    }
    tick(MAX_CHECK_AGE).await;
    for app in &apps {
        let (status, body) = response(app, "/ready").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["dependency"]["reason"], "stale");
        assert_eq!(body["dependency"]["age_ms"], 5000);
        assert_eq!(response(app, "/livez").await.0, StatusCode::OK);
    }
}

#[tokio::test(start_paused = true)]
async fn readiness_reports_only_durable_public_release_digests() {
    use layer3_adapter::{
        ReleasedLaneRevisions, ReleasedRuntime, ReleasedRuntimeSource, StaleReleasedRuntime,
    };

    #[derive(Debug)]
    struct Released(ReleasedRuntime);

    impl ReleasedRuntimeSource for Released {
        fn snapshot(&self) -> ReleasedRuntime {
            self.0.clone()
        }

        fn require_current(&self, release: &ReleasedRuntime) -> Result<(), StaleReleasedRuntime> {
            if *release == self.0 {
                Ok(())
            } else {
                Err(StaleReleasedRuntime)
            }
        }
    }

    let (_sender, state) = watch::channel(Some(CompletedCheck {
        completed: Instant::now(),
        checked_at: "2026-11-05T00:00:00Z".into(),
        duration_ms: 5,
        failure: None,
        diagnostic_check: 0,
    }));
    let digest = format!("sha256:{}", "d".repeat(64));
    let public_agent = format!("ghcr.io/chronoaiproject/cma-runtime@{digest}");
    let custom_machine = format!("ghcr.io/example/fixture@{digest}");
    let released = ReleasedRuntime {
        revision: 3,
        agent: public_agent,
        machine: Some(custom_machine),
        lane_revisions: Some(ReleasedLaneRevisions {
            agent: 2,
            machine: Some(1),
        }),
    };
    let app = |released| {
        health_routes(HealthState {
            store: state.clone(),
            workers: None,
            diagnostics: None,
            backend_free: false,
            runtime_release: Some(Arc::new(Released(released))),
        })
    };
    let (status, body) = response(&app(released.clone()), "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["runtime_release"],
        serde_json::json!({"agent": {"digest": digest, "revision": 2}, "machine": null})
    );
    let static_release = ReleasedRuntime {
        lane_revisions: None,
        ..released.clone()
    };
    assert!(response(&app(static_release), "/ready")
        .await
        .1
        .get("runtime_release")
        .is_none());
    tick(MAX_CHECK_AGE).await;
    let (status, body) = response(&app(released), "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.get("runtime_release").is_none());
}

#[tokio::test(start_paused = true)]
async fn dropping_the_router_cancels_the_inflight_check_and_stops_retries() {
    let probe = Arc::new(Probe::default());
    probe.mode.store(2, Ordering::SeqCst);
    let app = router(probe.clone());
    tokio::task::yield_now().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 1);
    drop(app);
    tokio::task::yield_now().await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    tick(Duration::from_secs(30)).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn execution_capability_is_separate_from_store_and_optional_warm_capacity() {
    let probe = Arc::new(Probe::default());
    let workers = crate::worker_supervision::Workers::new();
    let app = router_with_workers(probe.clone(), workers.clone(), None);
    tokio::task::yield_now().await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason"], "workers");
    for worker in workers.snapshot().iter().filter(|w| w.required) {
        let activity = workers.get(worker.worker);
        // Construction without a completed scheduling turn is insufficient.
        activity.waiting(Duration::from_secs(1), "idle_scan");
    }
    assert_eq!(
        response(&app, "/ready").await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    for worker in workers.snapshot().iter().filter(|w| w.required) {
        let activity = workers.get(worker.worker);
        activity.scheduled();
        activity.waiting(Duration::from_secs(1), "execution_slots");
    }
    assert_eq!(response(&app, "/ready").await.0, StatusCode::OK);
    assert_eq!(
        workers.get("native_warm_pools").snapshot().state,
        "starting"
    );
    let calls = probe.calls.load(Ordering::SeqCst);
    for _ in 0..50 {
        assert_eq!(response(&app, "/ready").await.0, StatusCode::OK);
    }
    assert_eq!(probe.calls.load(Ordering::SeqCst), calls);
    tick(Duration::from_secs(6)).await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason"], "workers");
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
    for worker in workers.snapshot().iter().filter(|w| w.required) {
        workers
            .get(worker.worker)
            .waiting(Duration::from_secs(1), "idle_scan");
    }
    assert_eq!(response(&app, "/ready").await.0, StatusCode::OK);
    probe.mode.store(1, Ordering::SeqCst);
    tick(CHECK_INTERVAL).await;
    let (status, body) = response(&app, "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["reason"], "store");
    assert!(workers.snapshot().iter().all(|w| w.available()));
    assert_eq!(response(&app, "/livez").await.0, StatusCode::OK);
}
