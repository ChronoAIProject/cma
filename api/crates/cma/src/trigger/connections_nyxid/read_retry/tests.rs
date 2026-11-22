use super::*;
use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::IntoResponse,
    Router,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: &'static str,
    delay: Duration,
    body_delay: Duration,
}

impl Reply {
    fn status(status: StatusCode) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: "{\"services\":[]}",
            delay: Duration::ZERO,
            body_delay: Duration::ZERO,
        }
    }

    fn throttle(hints: &[&str]) -> Self {
        let mut reply = Self::status(StatusCode::TOO_MANY_REQUESTS);
        for hint in hints {
            reply
                .headers
                .append(header::RETRY_AFTER, hint.parse().unwrap());
        }
        reply
    }
}

#[derive(Default)]
struct Provider {
    replies: Mutex<VecDeque<Reply>>,
    reads: Mutex<Vec<(Method, String, String, Instant)>>,
    observed: Notify,
}

struct Fixture {
    provider: Arc<Provider>,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        let provider = Arc::new(Provider::default());
        *provider.replies.lock().unwrap() = replies.into();
        let app = Router::new().fallback(handle).with_state(provider.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            provider,
            base,
            task,
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        reqwest::Client::new()
            .request(method, format!("{}{path}", self.base))
            .bearer_auth("synthetic-loopback-credential")
    }

    fn calls(&self) -> usize {
        self.provider.reads.lock().unwrap().len()
    }
}

async fn handle(
    State(provider): State<Arc<Provider>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    provider.reads.lock().unwrap().push((
        request.method().clone(),
        request.uri().to_string(),
        request.headers()[header::AUTHORIZATION]
            .to_str()
            .unwrap()
            .to_owned(),
        Instant::now(),
    ));
    provider.observed.notify_one();
    let reply = provider
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| Reply::status(StatusCode::INTERNAL_SERVER_ERROR));
    tokio::time::sleep(reply.delay).await;
    let body = axum::body::Body::from_stream(futures_util::stream::once(async move {
        tokio::time::sleep(reply.body_delay).await;
        Ok::<_, std::io::Error>(reply.body)
    }));
    (reply.status, reply.headers, body).into_response()
}

#[tokio::test]
async fn requirement_gets_retry_once_with_the_exact_request_after_provider_floor() {
    for path in [
        "/api/v1/user-services",
        "/api/v1/keys/key-one",
        "/api/v1/assistant/readiness",
    ] {
        let fixture =
            Fixture::new(vec![Reply::throttle(&["1"]), Reply::status(StatusCode::OK)]).await;
        let response = send(fixture.request(Method::GET, path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let reads = fixture.provider.reads.lock().unwrap();
        assert_eq!(reads.len(), 2);
        assert_eq!(
            (&reads[0].0, &reads[0].1, &reads[0].2),
            (&reads[1].0, &reads[1].1, &reads[1].2)
        );
        assert!(reads[1].3.duration_since(reads[0].3) >= Duration::from_secs(1));
    }
}

#[tokio::test]
async fn absent_or_invalid_retry_after_uses_a_one_second_floor() {
    for hints in [vec![], vec!["invalid"], vec!["-1"]] {
        let fixture =
            Fixture::new(vec![Reply::throttle(&hints), Reply::status(StatusCode::OK)]).await;
        assert_eq!(
            send(fixture.request(Method::GET, "/api/v1/user-services"))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let reads = fixture.provider.reads.lock().unwrap();
        assert!(reads[1].3.duration_since(reads[0].3) >= Duration::from_secs(1));
    }
}

#[tokio::test]
async fn long_numeric_date_and_multiple_hints_are_never_shortened_to_fit() {
    let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(60));
    for hints in [
        vec!["60"],
        vec![date.as_str()],
        vec!["0", "60"],
        vec!["18446744073709551615"],
    ] {
        let fixture = Fixture::new(vec![Reply::throttle(&hints)]).await;
        let response = send(fixture.request(Method::GET, "/api/v1/user-services"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test]
async fn repeated_throttle_exhausts_after_two_attempts_even_with_zero_provider_delay() {
    let fixture = Fixture::new(vec![Reply::throttle(&["0"]), Reply::throttle(&["0"])]).await;
    assert_eq!(
        send(fixture.request(Method::GET, "/api/v1/user-services"))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let reads = fixture.provider.reads.lock().unwrap();
    assert_eq!(reads.len(), 2);
    assert!(reads[1].3.duration_since(reads[0].3) >= Duration::from_millis(40));
}

#[tokio::test]
async fn refusal_other_failures_and_nonrequirement_operations_never_retry() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let fixture = Fixture::new(vec![Reply::status(status)]).await;
        assert_eq!(
            send(fixture.request(Method::GET, "/api/v1/user-services"))
                .await
                .unwrap()
                .status(),
            status
        );
        assert_eq!(fixture.calls(), 1);
    }
    for (method, path) in [
        (Method::POST, "/api/v1/connect-links"),
        (Method::POST, "/api/v1/user-services"),
        (Method::GET, "/api/v1/connect-links/one"),
        (Method::GET, "/api/v1/services"),
        (Method::GET, "/api/v1/keys/"),
        (Method::GET, "/api/v1/keys/one/refresh"),
    ] {
        let fixture = Fixture::new(vec![Reply::throttle(&["0"])]).await;
        assert_eq!(
            send(fixture.request(method, path)).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test]
async fn successful_or_malformed_evidence_never_retries() {
    for body in ["{\"services\":[]}", "{broken"] {
        let mut reply = Reply::status(StatusCode::OK);
        reply.body = body;
        let fixture = Fixture::new(vec![reply]).await;
        let response = send(fixture.request(Method::GET, "/api/v1/user-services"))
            .await
            .unwrap();
        let result = super::super::response::decode::<serde_json::Value>(response).await;
        assert_eq!(result.is_ok(), body != "{broken");
        assert_eq!(fixture.calls(), 1);
    }
}

#[tokio::test]
async fn cancellation_during_backoff_has_no_detached_retry() {
    let fixture = Fixture::new(vec![Reply::throttle(&["1"]), Reply::status(StatusCode::OK)]).await;
    let request = fixture.request(Method::GET, "/api/v1/user-services");
    let task = tokio::spawn(send(request));
    fixture.provider.observed.notified().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(1050)).await;
    assert_eq!(fixture.calls(), 1);
}

#[tokio::test]
async fn retry_cannot_reset_the_original_deadline_or_request_timeout() {
    let fixture = Fixture::new(vec![Reply::throttle(&["1"])]).await;
    let response = send_with_budget(
        fixture.request(Method::GET, "/api/v1/user-services"),
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(fixture.calls(), 1);
    let mut slow = Reply::status(StatusCode::OK);
    slow.delay = Duration::from_secs(2);
    let fixture = Fixture::new(vec![slow]).await;
    let started = Instant::now();
    assert!(send(
        fixture
            .request(Method::GET, "/api/v1/user-services")
            .timeout(Duration::from_millis(100))
    )
    .await
    .is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(fixture.calls(), 1);
}

#[tokio::test]
async fn retry_headers_and_body_must_complete_within_the_original_budget() {
    for body_timeout in [false, true] {
        let mut first = Reply::throttle(&["0"]);
        first.delay = Duration::from_millis(100);
        let mut slow = Reply::status(StatusCode::OK);
        if body_timeout {
            slow.body_delay = Duration::from_secs(2);
        } else {
            slow.delay = Duration::from_secs(2);
        }
        let fixture = Fixture::new(vec![first, slow]).await;
        let started = Instant::now();
        let result = send_with_budget(
            fixture.request(Method::GET, "/api/v1/user-services"),
            Duration::from_secs(1),
        )
        .await;
        if body_timeout {
            assert!(
                super::super::response::decode::<serde_json::Value>(result.unwrap())
                    .await
                    .is_err()
            );
        } else {
            assert!(result.is_err());
        }
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert_eq!(fixture.calls(), 2);
    }
}

#[tokio::test]
async fn transport_failure_is_not_replayed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/v1/user-services",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        drop(stream);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(send(reqwest::Client::new().get(url)).await.is_err());
    server.await.unwrap();
}
