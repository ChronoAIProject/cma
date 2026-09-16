use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Clone)]
struct TestState {
    requests: Arc<Mutex<Vec<Value>>>,
    paths: Arc<Mutex<Vec<String>>>,
    status: StatusCode,
    body: Value,
    delay: Duration,
    redirect: bool,
    retry_after: Option<String>,
}
async fn handle(
    State(state): State<TestState>,
    uri: axum::extract::OriginalUri,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers.get("authorization").unwrap(), "Bearer nyx_fixture");
    state.requests.lock().unwrap().push(body);
    state.paths.lock().unwrap().push(uri.0.path().to_owned());
    tokio::time::sleep(state.delay).await;
    let mut response = (state.status, Json(state.body)).into_response();
    if let Some(value) = state.retry_after {
        response
            .headers_mut()
            .insert("retry-after", value.parse().unwrap());
    }
    if state.redirect {
        response
            .headers_mut()
            .insert("location", "/unexpected-destination".parse().unwrap());
    }
    response
}
struct Fixture {
    provider: NyxBridgeProvider,
    state: TestState,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn fixture(status: StatusCode, body: Value, delay: Duration) -> Fixture {
    fixture_with_retry_after(status, body, delay, None).await
}
async fn fixture_with_retry_after(
    status: StatusCode,
    body: Value,
    delay: Duration,
    retry_after: Option<&str>,
) -> Fixture {
    let state = TestState {
        requests: Arc::new(Mutex::new(Vec::new())),
        paths: Arc::new(Mutex::new(Vec::new())),
        status,
        body,
        delay,
        redirect: status.is_redirection(),
        retry_after: retry_after.map(str::to_owned),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/v1/channel-relay/reply", post(handle))
        .route("/api/v1/channel-relay/reply/update", post(handle))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Fixture {
        provider: NyxBridgeProvider::new(&endpoint).unwrap(),
        state,
        server,
    }
}
async fn send(fixture: &Fixture, deadline: Duration) -> DeliveryOutcome {
    fixture
        .provider
        .post_reply(
            "original-inbound",
            "Progress update",
            &SecretValue::new("nyx_fixture".into()),
            tokio::time::Instant::now() + deadline,
        )
        .await
}

#[tokio::test]
async fn nyxid_reply_uses_original_inbound_and_returns_distinct_outbound_receipt() {
    let fixture = fixture(
        StatusCode::OK,
        json!({"message_id":"new-outbound","platform_message_id":"platform-reply"}),
        Duration::ZERO,
    )
    .await;
    assert_eq!(
        send(&fixture, Duration::from_secs(1)).await,
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "new-outbound".into(),
            platform_message_id: Some("platform-reply".into()),
        })
    );
    assert_eq!(
        *fixture.state.requests.lock().unwrap(),
        vec![json!({"message_id":"original-inbound","reply":{"text":"Progress update"}})]
    );
}

#[tokio::test]
async fn provider_failure_token_consumption_and_redirects_never_trigger_automatic_retry() {
    for status in [
        StatusCode::BAD_REQUEST,
        StatusCode::UNAUTHORIZED,
        StatusCode::CONFLICT,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::TEMPORARY_REDIRECT,
    ] {
        let fixture = fixture(
            status,
            json!({"error":"do not expose provider text"}),
            Duration::ZERO,
        )
        .await;
        assert_eq!(
            send(&fixture, Duration::from_secs(1)).await,
            DeliveryOutcome::Unknown(DeliveryFailure::ProviderFailure)
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn successful_http_without_valid_new_reply_receipt_is_unknown() {
    for body in [
        json!({}),
        json!({"message_id":""}),
        json!({"message_id":"original-inbound"}),
        json!({"message_id":"new","platform_message_id":""}),
        json!({"message_id":"new","unexpected":"field"}),
    ] {
        let fixture = fixture(StatusCode::OK, body, Duration::ZERO).await;
        assert_eq!(
            send(&fixture, Duration::from_secs(1)).await,
            DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid)
        );
        assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn lost_response_after_request_admission_is_unknown_and_not_retried() {
    let fixture = fixture(
        StatusCode::OK,
        json!({"message_id":"outbound"}),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(
        send(&fixture, Duration::from_millis(100)).await,
        DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)
    );
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn oversized_success_body_is_unknown() {
    let fixture = fixture(
        StatusCode::OK,
        json!({"message_id":"x".repeat(5000)}),
        Duration::ZERO,
    )
    .await;
    assert_eq!(
        send(&fixture, Duration::from_secs(1)).await,
        DeliveryOutcome::Unknown(DeliveryFailure::ReceiptInvalid)
    );
}

#[tokio::test]
async fn throttled_post_retains_unknown_outcome_and_backoff_without_a_second_request() {
    let fixture = fixture_with_retry_after(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":"private provider details"}),
        Duration::ZERO,
        Some("120"),
    )
    .await;
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let DeliveryOutcome::RateLimitedUnknown { not_before_ms } =
        send(&fixture, Duration::from_secs(1)).await
    else {
        panic!("rate metadata must not claim the original POST was not sent");
    };
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((before + 120_000..=after + 120_000).contains(&not_before_ms));
    assert_eq!(fixture.state.requests.lock().unwrap().len(), 1);
}

#[path = "edit_tests.rs"]
mod edits;
