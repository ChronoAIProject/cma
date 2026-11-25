//! Exercise the production proxy constructor and its complete request deadline.
use super::*;
use axum::body::{to_bytes, Bytes};
use layer3_adapter::RuntimeCredential;
use serde_json::json;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tower::ServiceExt as _;

struct Exchange {
    request: Request,
    reply: oneshot::Sender<Response>,
}

struct Peer {
    proxy: Router,
    origin: String,
    credential: CredentialHolder,
    inbox: mpsc::Receiver<Exchange>,
    count: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Peer {
    async fn start() -> Self {
        let (sender, inbox) = mpsc::channel(8);
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let app = Router::new().fallback(any(move |request: Request| {
            let sender = sender.clone();
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                let (reply, response) = oneshot::channel();
                sender.send(Exchange { request, reply }).await.unwrap();
                response
                    .await
                    .unwrap_or_else(|_| Response::new(Body::empty()))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let target =
            TargetBase::parse(&format!("{origin}/api/v1/proxy/s/chrono-llm-public")).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let credential = CredentialHolder::new();
        credential
            .install(&CredentialFrame {
                operation_id: "contract-install".into(),
                value: RuntimeCredential::new("contract-scoped-fixture"),
                custody_generation: Some(1),
                github_route: None,
            })
            .await
            .unwrap();
        Self {
            proxy: proxy_router(target, credential.clone(), None, true, false),
            origin,
            credential,
            inbox,
            count,
            server,
        }
    }

    fn send(&self) -> tokio::task::JoinHandle<Response> {
        self.send_request(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header(header::AUTHORIZATION, "Bearer caller-fixture")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"model":"fixture","input":"synthetic"}"#))
                .unwrap(),
        )
    }

    fn send_request(&self, request: Request) -> tokio::task::JoinHandle<Response> {
        let app = self.proxy.clone();
        tokio::spawn(async move { app.oneshot(request).await.unwrap() })
    }

    async fn stop(mut self) {
        self.server.abort();
        assert!((&mut self.server)
            .await
            .is_err_and(|error| error.is_cancelled()));
    }

    async fn next(&mut self) -> Exchange {
        tokio::time::timeout(Duration::from_secs(2), self.inbox.recv())
            .await
            .expect("local provider request arrives")
            .unwrap()
    }

    async fn assert_no_retry(&self) {
        tokio::time::advance(Duration::from_secs(1200)).await;
        tokio::task::yield_now().await;
        assert_eq!(self.count.load(Ordering::SeqCst), 1);
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn finish<T>(mut task: tokio::task::JoinHandle<T>) -> T {
    let result = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
    if result.is_err() {
        task.abort();
    }
    result
        .expect("proxy finishes within fixture watchdog")
        .unwrap()
}

fn assert_deadline(deadline: tokio::time::Instant) {
    assert!(
        tokio::time::Instant::now().saturating_duration_since(deadline) <= Duration::from_millis(1)
    );
}

#[tokio::test]
async fn proxy_preserves_success_and_typed_503_without_replaying_the_model_request() {
    for status in [200, 503] {
        let mut peer = Peer::start().await;
        let task = peer.send();
        let Exchange { request, reply } = peer.next().await;
        assert_eq!(request.method(), "POST");
        assert_eq!(
            request.uri(),
            "/api/v1/proxy/s/chrono-llm-public/v1/responses"
        );
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Bearer contract-scoped-fixture"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(request.into_body(), 4096).await.unwrap()
            )
            .unwrap(),
            json!({"model":"fixture","input":"synthetic"})
        );
        let payload = if status == 200 {
            json!({"output":[]})
        } else {
            json!({"error":{"code":"upstream_unavailable","message":"synthetic outage"}})
        };
        reply
            .send(
                Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::RETRY_AFTER, "3")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .unwrap();
        let response = finish(task).await;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::RETRY_AFTER], "3");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &to_bytes(response.into_body(), 4096).await.unwrap()
            )
            .unwrap(),
            payload
        );
        tokio::time::pause();
        peer.assert_no_retry().await;
        tokio::time::resume();
    }
}

#[tokio::test]
async fn absent_response_headers_end_with_fixed_502_at_the_production_deadline() {
    let mut peer = Peer::start().await;
    let task = peer.send();
    let held = peer.next().await;
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    tokio::time::advance(Duration::from_secs(600)).await;
    let response = finish(task).await;
    assert_deadline(deadline);
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        to_bytes(response.into_body(), 4096).await.unwrap(),
        "upstream request failed"
    );
    peer.assert_no_retry().await;
    drop(held);
}

#[tokio::test]
async fn accepted_stream_stall_ends_with_a_body_error_not_fabricated_success_or_retry() {
    let mut peer = Peer::start().await;
    let task = peer.send();
    let exchange = peer.next().await;
    exchange
        .reply
        .send(
            Response::builder()
                .status(200)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(futures_util::stream::pending::<
                    std::result::Result<Bytes, std::io::Error>,
                >()))
                .unwrap(),
        )
        .unwrap();
    // Receiving the response is the barrier proving headers reached the proxy.
    let response = finish(task).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::spawn(to_bytes(response.into_body(), 4096));
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    tokio::time::advance(Duration::from_secs(600)).await;
    assert!(finish(body).await.is_err());
    assert_deadline(deadline);
    peer.assert_no_retry().await;
}

#[path = "upstream_nyxid_contract_tests/h2_peer.rs"]
mod h2_peer;
#[path = "upstream_nyxid_contract_tests/h2_tests.rs"]
mod h2_tests;
#[path = "upstream_nyxid_contract_tests/route_tests.rs"]
mod route_tests;
