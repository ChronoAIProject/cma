use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Clone)]
struct TestState {
    requests: Arc<Mutex<Vec<Value>>>,
    status: StatusCode,
    body: Value,
    delay: Duration,
    redirect: bool,
}
async fn handle(
    State(state): State<TestState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    assert_eq!(headers.get("authorization").unwrap(), "Bearer nyx_fixture");
    state.requests.lock().unwrap().push(body);
    tokio::time::sleep(state.delay).await;
    let mut response = (state.status, Json(state.body)).into_response();
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
    let state = TestState {
        requests: Arc::new(Mutex::new(Vec::new())),
        status,
        body,
        delay,
        redirect: status.is_redirection(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/v1/channel-relay/reply", post(handle))
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
        StatusCode::TOO_MANY_REQUESTS,
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
