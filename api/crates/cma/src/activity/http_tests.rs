use super::{
    queue::{ActivityQueue, ActivitySink},
    ActivityRecorder,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
    Router,
};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Page, StoreError,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
use tower::ServiceExt;

#[derive(Default)]
struct Store {
    calls: AtomicUsize,
    records: Mutex<Vec<ActivityRecord>>,
    delay: Duration,
    fail: bool,
}
#[async_trait::async_trait]
impl ActivityStore for Store {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        if self.fail {
            return Err(StoreError::Unavailable("injected".into()));
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(self.records.lock().unwrap().clone(), None))
    }
}

fn app(recorder: Option<Arc<ActivityRecorder>>) -> Router {
    let app = Router::new()
        .route(
            "/api/v1/agents/{id}/history",
            get(|| async { StatusCode::OK }),
        )
        .route("/livez", get(|| async { StatusCode::OK }))
        .route("/ready", get(|| async { StatusCode::OK }))
        .route("/denied", get(|| async { StatusCode::FORBIDDEN }));
    let app = match recorder {
        Some(recorder) => app.layer(middleware::from_fn_with_state(recorder, super::record)),
        None => app,
    };
    // Production's outer read budget must release its slots before Mongo finishes.
    app.layer(middleware::from_fn_with_state(
        crate::expensive_reads::ReadBudget::new(false),
        crate::expensive_reads::enforce,
    ))
}

async fn request(app: Router, path: &str) -> StatusCode {
    let response = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    status
}

#[tokio::test]
async fn thousand_responses_stay_within_budget_during_thirty_second_storage_stall() {
    let store = Arc::new(Store {
        delay: Duration::from_secs(30),
        ..Store::default()
    });
    let (queue, worker) = ActivityQueue::new(store.clone());
    let app = app(Some(ActivityRecorder::with_sink(
        ActivityService::new(store.clone()),
        queue.clone(),
    )));
    let baseline = self::app(None);
    let (stop, signal) = tokio::sync::watch::channel(false);
    let worker = worker.spawn(signal);
    // Start a batch before sampling so every measured request overlaps the stall.
    for _ in 0..256 {
        assert!(queue.try_enqueue(ActivityRecord::new(
            layer2_core::ActivityAction::Read,
            layer2_core::ActivityOutcome::Succeeded
        )));
    }
    for _ in 0..100 {
        if store.calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    let mut increments = Vec::new();
    for _ in 0..1000 {
        let before = Instant::now();
        assert_eq!(
            request(baseline.clone(), "/api/v1/agents/a/history").await,
            StatusCode::OK
        );
        let base = before.elapsed();
        let before = Instant::now();
        assert_eq!(
            request(app.clone(), "/api/v1/agents/a/history").await,
            StatusCode::OK
        );
        increments.push(before.elapsed().saturating_sub(base));
    }
    increments.sort();
    eprintln!(
        "activity async blocked-store: n=1000 p99_increment={:?} max_increment={:?}",
        increments[989], increments[999]
    );
    assert!(increments[989] <= Duration::from_millis(50));
    assert!(increments[999] <= Duration::from_millis(100));
    assert!(store.records.lock().unwrap().is_empty());
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    stop.send(true).unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn disabled_mode_still_waits_and_preserves_failure_status_and_health_exclusions() {
    for fail in [false, true] {
        let store = Arc::new(Store {
            delay: Duration::from_secs(1),
            fail,
            ..Store::default()
        });
        let app = app(Some(ActivityRecorder::new(ActivityService::new(
            store.clone(),
        ))));
        for path in ["/livez", "/ready"] {
            assert_eq!(request(app.clone(), path).await, StatusCode::OK);
        }
        assert_eq!(store.calls.load(Ordering::SeqCst), 0);
        let start = tokio::time::Instant::now();
        assert_eq!(request(app, "/denied").await, StatusCode::FORBIDDEN);
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        assert_eq!(store.calls.load(Ordering::SeqCst), 1);
        let records = store.records.lock().unwrap();
        assert_eq!(records.len(), usize::from(!fail));
        if !fail {
            assert_eq!(
                records[0].outcome,
                layer2_core::ActivityOutcome::Refused("403".into())
            );
        }
    }
}

struct HeldAudit {
    entered: Semaphore,
    release: Semaphore,
    fail: bool,
}
#[async_trait::async_trait]
impl ActivityStore for HeldAudit {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        if self.fail {
            Err(StoreError::Unavailable("audit failed".into()))
        } else {
            Ok(())
        }
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn explicit_activity_service_still_waits_and_propagates_storage_failure_with_queue_enabled() {
    for fail in [false, true] {
        let store = Arc::new(HeldAudit {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            fail,
        });
        let service = ActivityService::new(store.clone());
        let (sink, _worker) = ActivityQueue::new(store.clone());
        let _recorder = ActivityRecorder::with_sink(service.clone(), sink);
        let audit = tokio::spawn(async move {
            service
                .record(&ActivityRecord::new(
                    layer2_core::ActivityAction::Configure,
                    layer2_core::ActivityOutcome::Succeeded,
                ))
                .await
        });
        store.entered.acquire().await.unwrap().forget();
        assert!(!audit.is_finished());
        store.release.add_permits(1);
        assert_eq!(audit.await.unwrap().is_err(), fail);
    }
}

#[tokio::test(start_paused = true)]
async fn async_mode_preserves_event_time_status_actor_and_health_exclusions() {
    let store = Arc::new(Store::default());
    let (sink, worker) = ActivityQueue::new(store.clone());
    let app = app(Some(ActivityRecorder::with_sink(
        ActivityService::new(store.clone()),
        sink,
    )));
    for path in ["/livez", "/ready"] {
        assert_eq!(request(app.clone(), path).await, StatusCode::OK);
    }
    let mut req = Request::builder()
        .uri("/denied")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            layer2_core::SubjectId::new("actor-test"),
        ));
    let start = layer2_core::now_rfc3339();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let end = layer2_core::now_rfc3339();
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
    shutdown.push(worker.spawn(shutdown.subscribe()));
    assert_eq!(shutdown.drain().await, 0);
    let records = store.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].actor,
        Some(layer2_core::SubjectId::new("actor-test"))
    );
    assert_eq!(
        records[0].outcome,
        layer2_core::ActivityOutcome::Refused("403".into())
    );
    assert!(records[0].at >= start && records[0].at <= end);
}

#[tokio::test]
async fn configured_audit_sampling_preserves_default_reads_and_all_refusals_mutations_domain_audits(
) {
    use axum::http::Method;
    use layer2_core::{ActivityAction, ActivityOutcome, SubjectId};
    for sampling in [false, true] {
        let store = Arc::new(Store::default());
        let service = ActivityService::new(store.clone());
        let router = Router::new()
            .route(
                "/api/v1/sandboxes/{id}",
                get(|| async { StatusCode::OK }).post(|| async { StatusCode::CREATED }),
            )
            .route(
                "/api/v1/workspaces/{id}",
                get(|| async { StatusCode::FORBIDDEN }),
            )
            .route("/api/v1/admin/activity", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(
                ActivityRecorder::configured(service.clone(), None, sampling),
                super::record,
            ));
        let cases = [
            (
                Method::POST,
                "/api/v1/sandboxes/fixture",
                StatusCode::CREATED,
            ),
            (
                Method::GET,
                "/api/v1/workspaces/fixture",
                StatusCode::FORBIDDEN,
            ),
            (Method::GET, "/api/v1/admin/activity", StatusCode::OK),
        ];
        for (method, path, status) in cases {
            let mut request = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            request
                .extensions_mut()
                .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
                    "fixture",
                )));
            assert_eq!(
                router.clone().oneshot(request).await.unwrap().status(),
                status
            );
        }
        assert_eq!(store.records.lock().unwrap().len(), 3);
        if !sampling {
            for method in [Method::GET, Method::HEAD] {
                let mut request = Request::builder()
                    .method(method)
                    .uri("/api/v1/sandboxes/fixture")
                    .body(Body::empty())
                    .unwrap();
                request
                    .extensions_mut()
                    .insert(layer1_public::AuthenticatedSubject(SubjectId::new(
                        "fixture",
                    )));
                assert_eq!(
                    router.clone().oneshot(request).await.unwrap().status(),
                    StatusCode::OK
                );
            }
            assert_eq!(store.records.lock().unwrap().len(), 5);
        }
        // Domain/security writers continue to use the unfiltered durable port.
        let before = store.records.lock().unwrap().len();
        service
            .record(&ActivityRecord::new(
                ActivityAction::Authenticate,
                ActivityOutcome::Succeeded,
            ))
            .await
            .unwrap();
        assert_eq!(store.records.lock().unwrap().len(), before + 1);
    }
}

#[tokio::test]
async fn typed_usage_view_probe_preserves_legacy_success_and_other_failures_in_audit() {
    use axum::{response::IntoResponse, routing::post};
    use layer1_public::rest::UsageReportViewDisabled;
    use layer2_core::ActivityOutcome;

    async fn disabled() -> impl IntoResponse {
        (
            axum::Extension(UsageReportViewDisabled),
            StatusCode::NOT_IMPLEMENTED,
        )
    }

    let store = Arc::new(Store::default());
    let app = Router::new()
        .route("/api/v1/admin/usage/metrics/view", get(disabled))
        .route("/api/v1/admin/usage/metrics/initial/view", post(disabled))
        .route(
            "/api/v1/admin/usage/metrics/initial/reports",
            post(disabled),
        )
        .route(
            "/api/v1/admin/usage/metrics/reports",
            post(|| async { StatusCode::ACCEPTED }),
        )
        .route("/generic", get(|| async { StatusCode::NOT_IMPLEMENTED }))
        .route(
            "/unavailable",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        )
        .route("/forbidden", get(|| async { StatusCode::FORBIDDEN }))
        .layer(middleware::from_fn_with_state(
            ActivityRecorder::new(ActivityService::new(store.clone())),
            super::record,
        ));

    for (method, path, status) in [
        (
            "GET",
            "/api/v1/admin/usage/metrics/view",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "POST",
            "/api/v1/admin/usage/metrics/initial/view",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "POST",
            "/api/v1/admin/usage/metrics/reports",
            StatusCode::ACCEPTED,
        ),
        (
            "POST",
            "/api/v1/admin/usage/metrics/initial/reports",
            StatusCode::NOT_IMPLEMENTED,
        ),
        ("GET", "/generic", StatusCode::NOT_IMPLEMENTED),
        ("GET", "/unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("GET", "/forbidden", StatusCode::FORBIDDEN),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    let records = store.records.lock().unwrap();
    let outcomes: Vec<_> = records.iter().map(|record| &record.outcome).collect();
    assert_eq!(
        outcomes,
        [
            &ActivityOutcome::Succeeded,
            &ActivityOutcome::Failed("501".into()),
            &ActivityOutcome::Failed("501".into()),
            &ActivityOutcome::Failed("503".into()),
            &ActivityOutcome::Refused("403".into()),
        ]
    );
}
