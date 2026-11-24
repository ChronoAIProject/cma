use super::*;
use axum::{http::StatusCode, routing::post, Json, Router};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) async fn serve_fixture(
    router: Router,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (stop, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    (base, stop, task)
}

pub(super) async fn record_probe(
    state: &mut ManagementState,
    status: StatusCode,
) -> (
    Arc<AtomicUsize>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let router = Router::new().fallback(move || {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            (
                status,
                "synthetic upstream body: owner=private-owner; credential=private-value",
            )
        }
    });
    let (base, stop, task) = serve_fixture(router).await;
    state.records = Arc::new(RecordClient::new(&base, "cma-service").unwrap());
    state.cma_base_url = base;
    (calls, stop, task)
}

pub(super) async fn problem(response: reqwest::Response, status: u16, code: &str) -> Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], status);
    assert_eq!(body["code"], code);
    body
}

#[tokio::test]
async fn recovery_rejects_malformed_record_ids_before_any_record_or_provider_request() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
    let (calls, probe_stop, probe_task) =
        record_probe(&mut state, StatusCode::SERVICE_UNAVAILABLE).await;
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    for id in [
        "not-a-uuid".to_owned(),
        "00000000-0000-0000-0000-000000000000".into(),
        "12345678-1234-1234-8123-123456789abc".into(),
        "12345678-1234-7234-8123-123456789abc".into(),
        "12345678-1234-4234-8123-123456789ABC".into(),
        "12345678123442348123123456789abc".into(),
        "{12345678-1234-4234-8123-123456789abc}".into(),
        "urn:uuid:12345678-1234-4234-8123-123456789abc".into(),
        "x".repeat(513),
    ] {
        let mut url = reqwest::Url::parse(&format!("{base}/v1/channel-records/")).unwrap();
        url.path_segments_mut().unwrap().pop_if_empty().push(&id);
        let response = client
            .get(url)
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        let body = problem(response, 400, "invalid_request").await;
        assert_eq!(
            body,
            json!({
                "type":"about:blank", "title":"invalid request", "status":400,
                "detail":"/record_id: must be a canonical UUID v4", "code":"invalid_request"
            })
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(provider.log().is_empty());
    }
    let response = client
        .get(format!(
            "{base}/v1/channel-records/12345678-1234-4234-8123-123456789abc"
        ))
        .bearer_auth("token-alice")
        .send()
        .await
        .unwrap();
    problem(response, 503, "provider_unavailable").await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the probe observes valid record requests"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    probe_stop.send(()).unwrap();
    probe_task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn recovery_keeps_missing_records_and_real_store_failures_distinct() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let client = Client::new();
    for (upstream, status, code, detail) in [
        (
            StatusCode::NOT_FOUND,
            404,
            "channel_not_found",
            "The channel does not exist.",
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            503,
            "provider_unavailable",
            "record store: channel record store unavailable: HTTP 503 Service Unavailable",
        ),
        (
            StatusCode::BAD_REQUEST,
            503,
            "provider_unavailable",
            "record store: channel record request was rejected: HTTP 400 Bad Request",
        ),
    ] {
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        let (calls, probe_stop, probe_task) = record_probe(&mut state, upstream).await;
        let (base, stop, task) = serve_app(state).await;
        let response = client
            .get(format!(
                "{base}/v1/channel-records/12345678-1234-4234-8123-123456789abc"
            ))
            .bearer_auth("token-alice")
            .send()
            .await
            .unwrap();
        let body = problem(response, status, code).await;
        assert_eq!(body["detail"], detail);
        assert!(!body.to_string().contains("private-"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(provider.log().is_empty());
        stop.send(()).unwrap();
        task.await.unwrap();
        probe_stop.send(()).unwrap();
        probe_task.await.unwrap();
    }
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}

#[tokio::test]
async fn recovery_authentication_and_read_permission_precede_record_validation() {
    let provider = FakeProvider::start(FakeProviderConfig::default()).await;
    let cma = FakeCma::start(false).await;
    let nyxid = FakeNyxid::start().await;
    let auth = Router::new().route("/oauth/introspect", post(|headers: axum::http::HeaderMap| async move {
        let issuer = format!("http://{}", headers["host"].to_str().unwrap());
        Json(json!({
            "active":true, "sub":"alice", "iss":issuer,
            "exp":time::OffsetDateTime::now_utc().unix_timestamp() + 3600,
            "token_type":"access", "resource":[format!("{issuer}/api/v1/proxy/s/cma-event-gateway")],
            "permissions":["cma-eg:channels:create"]
        }))
    }));
    let (issuer, auth_stop, auth_task) = serve_fixture(auth).await;
    let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
    state.creator_auth = Arc::new(
        CreatorAuthenticator::new(&issuer).unwrap().with_service(
            cma_service_auth::NyxIdAuth::new(
                &issuer,
                cma_service_auth::Service::EventGateway,
                "gateway-management",
                "gateway-management-fixture".into(),
            )
            .unwrap(),
        ),
    );
    let (calls, probe_stop, probe_task) =
        record_probe(&mut state, StatusCode::SERVICE_UNAVAILABLE).await;
    let (base, stop, task) = serve_app(state).await;
    let client = Client::new();
    for id in ["not-a-uuid", "12345678-1234-4234-8123-123456789abc"] {
        let url = format!("{base}/v1/channel-records/{id}");
        problem(
            client.get(&url).send().await.unwrap(),
            401,
            "not_authenticated",
        )
        .await;
        problem(
            client
                .get(&url)
                .bearer_auth("token-without-read")
                .send()
                .await
                .unwrap(),
            403,
            "not_authenticated",
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(provider.log().is_empty());
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    probe_stop.send(()).unwrap();
    probe_task.await.unwrap();
    auth_stop.send(()).unwrap();
    auth_task.await.unwrap();
    provider.close().await;
    cma.close().await;
    nyxid.close().await;
}
