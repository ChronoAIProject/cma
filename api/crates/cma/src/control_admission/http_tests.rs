use super::*;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware,
    routing::{delete, post},
    Router,
};
use layer2_core::SubjectId;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

struct Rates(AtomicUsize);
#[async_trait::async_trait]
impl policy::SharedControlRate for Rates {
    async fn charge(&self, _: &str, _: u32, _: u32) -> Result<Option<u64>, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn router(enabled: bool) -> (Router, Arc<Rates>) {
    let store = Arc::new(Rates(AtomicUsize::new(0)));
    let policy: Arc<dyn LifecycleAdmissionPolicy> = Arc::new(Policy::new(
        ControlAdmissionConfig {
            enabled,
            ..Default::default()
        },
        store.clone(),
        Arc::new(SystemClock),
    ));
    let endpoint = |request: Request| async move {
        assert!(layer2_core::agent_ownership::execution_authority().is_none());
        if request.extensions().get::<Admitted>().is_some() {
            StatusCode::ACCEPTED
        } else {
            StatusCode::NO_CONTENT
        }
    };
    let router = Router::new()
        .route("/api/v1/agents/{id}", delete(endpoint).get(endpoint))
        .route("/api/v1/agents/{id}/stop", post(endpoint).get(endpoint))
        .route("/api/v1/workspaces/{id}/stop", post(endpoint))
        .layer(middleware::from_fn_with_state(policy, enforce));
    (router, store)
}

fn request(method: Method, uri: &str, caller: Option<&str>) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-cma-agent-control-admitted", "forged")
        .body(Body::empty())
        .unwrap();
    if let Some(caller) = caller {
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(SubjectId::new(caller)));
    }
    request
}

#[tokio::test]
async fn exact_routes_charge_by_authenticated_caller_and_never_accept_forged_marker() {
    let (router, rates) = router(true);
    for (method, uri, caller, expected) in [
        (
            Method::POST,
            "/api/v1/agents/a/stop",
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            Method::GET,
            "/api/v1/agents/a/stop",
            Some("one"),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::GET,
            "/api/v1/agents/a",
            Some("one"),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::POST,
            "/api/v1/workspaces/a/stop",
            Some("one"),
            StatusCode::NO_CONTENT,
        ),
        (
            Method::DELETE,
            "/api/v1/agents/a",
            Some("one"),
            StatusCode::ACCEPTED,
        ),
        (
            Method::DELETE,
            "/api/v1/agents/b",
            Some("one"),
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            Method::POST,
            "/api/v1/agents/a/stop",
            Some("one"),
            StatusCode::ACCEPTED,
        ),
        (
            Method::POST,
            "/api/v1/agents/b/stop",
            Some("one"),
            StatusCode::TOO_MANY_REQUESTS,
        ),
        (
            Method::POST,
            "/api/v1/agents/b/stop",
            Some("two"),
            StatusCode::ACCEPTED,
        ),
    ] {
        let response = router
            .clone()
            .oneshot(request(method.clone(), uri, caller))
            .await
            .unwrap();
        assert_eq!(response.status(), expected, "{method} {uri}");
        if expected == StatusCode::TOO_MANY_REQUESTS {
            assert!(response.headers().contains_key("retry-after"));
        }
    }
    assert_eq!(rates.0.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn flag_off_never_charges_and_preserves_the_existing_path() {
    let (router, rates) = router(false);
    for _ in 0..10 {
        assert_eq!(
            router
                .clone()
                .oneshot(request(Method::POST, "/api/v1/agents/a/stop", Some("one")))
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(rates.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn disconnected_http_observer_keeps_its_executing_stage_budgeted() {
    let store = Arc::new(Rates(AtomicUsize::new(0)));
    let policy = Arc::new(Policy::new(
        ControlAdmissionConfig {
            enabled: true,
            ..Default::default()
        },
        store,
        Arc::new(SystemClock),
    ));
    let started = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let handler_state = (started.clone(), finish.clone());
    let admitted: Arc<dyn LifecycleAdmissionPolicy> = policy.clone();
    let router = Router::new()
        .route(
            "/api/v1/agents/{id}/stop",
            post(
                |State((started, finish)): State<(
                    Arc<tokio::sync::Notify>,
                    Arc<tokio::sync::Notify>,
                )>| async move {
                    started.notify_one();
                    finish.notified().await;
                    StatusCode::ACCEPTED
                },
            ),
        )
        .with_state(handler_state)
        .layer(middleware::from_fn_with_state(admitted, enforce));
    let observer = tokio::spawn(router.clone().oneshot(request(
        Method::POST,
        "/api/v1/agents/a/stop",
        Some("one"),
    )));
    started.notified().await;
    observer.abort();
    assert!(observer.await.unwrap_err().is_cancelled());
    assert!(matches!(
        policy.resources().try_stage("one", "b", Class::Stop, 1),
        Err(Refusal::Caller(_))
    ));
    assert_eq!(
        router
            .oneshot(request(
                Method::POST,
                "/api/v1/agents/%61/stop",
                Some("two")
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS,
        "encoded aliases must share the same target slot"
    );
    finish.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if policy
                .resources()
                .try_stage("one", "b", Class::Stop, 1)
                .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn missing_registered_route_metadata_does_not_select_lifecycle_policy() {
    let bare = request(Method::POST, "/api/v1/agents/a/stop", Some("one"));
    assert!(classify(&bare).is_none());
}

#[tokio::test]
async fn delegated_router_and_public_stack_share_policy_without_double_charging() {
    let rates = Arc::new(Rates(AtomicUsize::new(0)));
    let policy: Arc<dyn LifecycleAdmissionPolicy> = Arc::new(Policy::new(
        ControlAdmissionConfig {
            enabled: true,
            ..Default::default()
        },
        rates.clone(),
        Arc::new(SystemClock),
    ));
    let resources = super::routes(
        Router::new().route(
            "/api/v1/agents/{id}",
            delete(|| async {
                let context = layer2_core::diagnostic_context::DiagnosticContext::current();
                assert_eq!(context.worker, "request");
                assert_eq!(&*context.route, "DELETE /api/v1/agents/{id}");
                StatusCode::ACCEPTED
            }),
        ),
        Some(policy.clone()),
    );
    let public = super::routes(resources.clone(), Some(policy));
    let context = layer2_core::diagnostic_context::DiagnosticContext {
        worker: "request",
        route: "DELETE /api/v1/agents/{id}".into(),
        ..Default::default()
    };
    assert_eq!(
        context
            .clone()
            .scope(public.oneshot(request(Method::DELETE, "/api/v1/agents/a", Some("one"))))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(rates.0.load(Ordering::SeqCst), 1);
    // A delegated dispatch carries verified extensions and takes the same
    // registered route. It cannot obtain another allowance through this entry.
    assert_eq!(
        context
            .clone()
            .scope(resources.clone().oneshot(request(
                Method::DELETE,
                "/api/v1/agents/a",
                Some("one")
            )))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        context
            .scope(resources.oneshot(request(Method::DELETE, "/api/v1/agents/b", Some("two"))))
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(rates.0.load(Ordering::SeqCst), 2);
}

struct RefusingPolicy(Refusal);
#[async_trait::async_trait]
impl LifecycleAdmissionPolicy for RefusingPolicy {
    fn enabled(&self) -> bool {
        true
    }
    async fn admit(&self, _: &str, _: &str, _: Class) -> Result<Option<StagePermit>, Refusal> {
        Err(self.0)
    }
}

#[tokio::test]
async fn service_capacity_and_quota_failure_reuse_the_existing_problem_response() {
    for refusal in [Refusal::Capacity, Refusal::Unavailable] {
        let policy: Arc<dyn LifecycleAdmissionPolicy> = Arc::new(RefusingPolicy(refusal));
        let app = super::routes(
            Router::new().route("/api/v1/agents/{id}", delete(failed_handler)),
            Some(policy),
        );
        let response = app
            .oneshot(request(Method::DELETE, "/api/v1/agents/a", Some("one")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "request_budget_unavailable");
    }
}

#[tokio::test]
async fn handler_failure_releases_capacity_and_returns_an_honest_unavailable_response() {
    let policy = Arc::new(Policy::new(
        ControlAdmissionConfig {
            enabled: true,
            ..Default::default()
        },
        Arc::new(Rates(AtomicUsize::new(0))),
        Arc::new(SystemClock),
    ));
    let app = super::routes(
        Router::new().route("/api/v1/agents/{id}/stop", post(failed_handler)),
        Some(policy.clone()),
    );
    assert_eq!(
        app.oneshot(request(Method::POST, "/api/v1/agents/a/stop", Some("one")))
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(policy
        .resources()
        .try_stage("one", "a", Class::Stop, 1)
        .is_ok());
}

async fn failed_handler() -> StatusCode {
    panic!("injected handler failure");
}
