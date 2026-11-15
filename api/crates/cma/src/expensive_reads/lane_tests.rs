use super::*;

const USAGE: &str = "/api/v1/agents/{id}/usage";
const VIEW: &str = "/api/v1/usage/metrics/view";

#[tokio::test]
async fn small_observations_and_reports_cannot_starve_history_or_each_other() {
    let budget = ReadBudget::new(false);
    let router = protected(
        Router::new()
            .route(HISTORY, get(|| async { "history" }))
            .route(USAGE, get(|| async { "usage" }))
            .route(VIEW, get(|| async { "view" })),
        budget.clone(),
    );
    let mut held = Vec::new();
    for (url, retry) in [("/api/v1/agents/a/usage", "1"), (VIEW, "2"), (URL, "1")] {
        for _ in 0..2 {
            let response = request(&router, Method::GET, url).await;
            assert_eq!(response.status(), StatusCode::OK);
            held.push(response);
        }
        let refused = request(&router, Method::GET, url).await;
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(refused.headers()[header::RETRY_AFTER], retry);
        assert_eq!(refused.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(refused.into_body(), 2048).await.unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "buffered_read_capacity_exhausted");
    }
    drop(held);
    for slot in [&budget.slots, &budget.observations, &budget.reports] {
        assert_eq!(slot.available_permits(), 2);
    }
}

#[tokio::test]
async fn exact_descriptor_templates_preserve_projection_and_mutation_contracts() {
    for (template, url) in [
        (VIEW, VIEW),
        (
            "/api/v1/admin/usage/metrics/view",
            "/api/v1/admin/usage/metrics/view",
        ),
        (
            "/api/v1/usage/metrics/reports/{report}",
            "/api/v1/usage/metrics/reports/r",
        ),
        (
            "/api/v1/admin/usage/metrics/reports/{report}",
            "/api/v1/admin/usage/metrics/reports/r",
        ),
    ] {
        let budget = ReadBudget::new(false);
        let _held = budget.reports.clone().try_acquire_many_owned(2).unwrap();
        let router = protected(
            Router::new().route(
                template,
                get(|| async { "ok" }).post(|| async { "created" }),
            ),
            budget,
        );
        for method in [Method::GET, Method::HEAD] {
            assert_eq!(
                request(&router, method, url).await.status(),
                StatusCode::TOO_MANY_REQUESTS
            );
        }
        assert_eq!(
            request(&router, Method::POST, url).await.status(),
            StatusCode::OK
        );
    }
    let budget = ReadBudget::new(false);
    let _held = budget.reports.clone().try_acquire_many_owned(2).unwrap();
    for suffix in [
        "series",
        "sandboxes",
        "owners",
        "owner-allocations",
        "sandboxes/{sandbox}",
    ] {
        let template = format!("/api/v1/admin/usage/metrics/reports/{{report}}/{suffix}");
        let url = template.replace("{report}", "r").replace("{sandbox}", "s");
        let router = protected(
            Router::new().route(&template, get(|| async { vec![0_u8; 1024 * 1024 + 1] })),
            budget.clone(),
        );
        assert_eq!(
            request(&router, Method::GET, &url).await.status(),
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn small_lanes_enforce_bytes_and_preserve_downstream_denials() {
    for (template, url, cap) in [
        (USAGE, "/api/v1/agents/a/usage", Lane::Observation.bytes()),
        (VIEW, VIEW, Lane::Report.bytes()),
    ] {
        let budget = ReadBudget::new(false);
        for (bytes, expected) in [
            (cap, StatusCode::OK),
            (cap + 1, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let router = protected(
                Router::new().route(template, get(move || async move { vec![0_u8; bytes] })),
                budget.clone(),
            );
            assert_eq!(request(&router, Method::GET, url).await.status(), expected);
        }
        let router = protected(
            Router::new().route(template, get(|| async { StatusCode::FORBIDDEN })),
            budget,
        );
        assert_eq!(
            request(&router, Method::GET, url).await.status(),
            StatusCode::FORBIDDEN
        );
    }
}

#[tokio::test(start_paused = true)]
async fn report_deadline_and_cancellation_return_only_their_lane_capacity() {
    let budget = ReadBudget::new(true);
    let entered = Arc::new(tokio::sync::Notify::new());
    let observer = entered.clone();
    let router = protected(
        Router::new().route(
            VIEW,
            get(move || {
                let entered = observer.clone();
                async move {
                    entered.notify_one();
                    std::future::pending::<&'static str>().await
                }
            }),
        ),
        budget.clone(),
    );
    let pending = router.clone();
    let task = tokio::spawn(async move { request(&pending, Method::GET, VIEW).await });
    entered.notified().await;
    assert_eq!(budget.reports.available_permits(), 1);
    assert_eq!(budget.slots.available_permits(), 2);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(budget.reports.available_permits(), 2);
    let started = tokio::time::Instant::now();
    assert_eq!(
        request(&router, Method::GET, VIEW).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert_eq!(budget.reports.available_permits(), 2);
}

#[tokio::test(start_paused = true)]
async fn small_lane_body_deadline_and_retained_frames_keep_independent_ownership() {
    let budget = ReadBudget::new(true);
    let streaming = protected(
        Router::new().route(
            VIEW,
            get(|| async {
                Body::from_stream(stream::pending::<Result<&'static str, Infallible>>())
            }),
        ),
        budget.clone(),
    );
    let started = tokio::time::Instant::now();
    assert_eq!(
        request(&streaming, Method::GET, VIEW).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    assert_eq!(budget.reports.available_permits(), 2);
    let router = protected(
        Router::new().route(VIEW, get(|| async { "descriptor" })),
        budget.clone(),
    );
    let body = request(&router, Method::GET, VIEW).await.into_body();
    let bytes = to_bytes(body, 1024).await.unwrap();
    assert_eq!(budget.reports.available_permits(), 1);
    assert_eq!(budget.observations.available_permits(), 2);
    assert_eq!(budget.slots.available_permits(), 2);
    drop(bytes);
    assert_eq!(budget.reports.available_permits(), 2);
}
