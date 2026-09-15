use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

#[derive(Default)]
struct DeniedBackend {
    authenticated: AtomicUsize,
}

struct DiscoveryBackend;
#[async_trait::async_trait]
impl ChannelToolBackend for DiscoveryBackend {
    async fn authorize_connection(&self, _: &RuntimeCredential) -> Result<(), OutboxError> {
        Ok(())
    }
    async fn authenticate(
        &self,
        _: &RuntimeCredential,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        Err(OutboxError::ToolUnavailable)
    }
    async fn context(
        &self,
        _: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError> {
        panic!("discovery cannot read active turn")
    }
    async fn reply(
        &self,
        _: &ChannelReplySessionAuthorization,
        _: &str,
        _: ChannelReplyToolRequest,
    ) -> Result<ReplyToolReceipt, OutboxError> {
        panic!("discovery cannot send")
    }
}

async fn discovery_request(value: Value) -> Response {
    router(Arc::new(DiscoveryBackend),"https://cma.example").oneshot(Request::builder().method("POST")
        .uri("/api/v1/channel-tools").header("content-type","application/json")
        .header("authorization","Bearer cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .body(axum::body::Body::from(value.to_string())).unwrap()).await.unwrap()
}

#[tokio::test]
async fn opening_session_can_negotiate_and_discover_before_publication_but_cannot_send() {
    let response=discovery_request(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"fixture","version":"1"},"capabilities":{}}})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");
    assert!(body["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("while you work"));
    let response = discovery_request(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).await;
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap(),
    )
    .unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[1]["name"], "reply_to_channel");
    assert_eq!(
        tools[1]["inputSchema"]["required"],
        json!(["reply_context", "request_key", "text"])
    );
    assert_eq!(tools[1]["inputSchema"]["additionalProperties"], false);
    assert_eq!(tools[2]["name"], "read_channel_control_context");
    assert_eq!(tools[2]["annotations"]["readOnlyHint"], true);
    assert_eq!(tools[2]["inputSchema"]["additionalProperties"], false);
    assert!(tools[2]["description"]
        .as_str()
        .unwrap()
        .contains("operations is empty"));
    let response=discovery_request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"reply_to_channel","arguments":{"reply_context":format!("brctx_{}","a".repeat(64)),"request_key":"ack-1","text":"Starting"}}})).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn notification_and_invalid_request_cannot_trigger_external_send() {
    let response = discovery_request(json!({"jsonrpc":"2.0","method":"tools/call",
        "params":{"name":"reply_to_channel","arguments":{"text":"must not send"}}}))
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    for request in [
        json!([{"jsonrpc":"2.0","id":1,"method":"tools/call"}]),
        json!({"jsonrpc":"2.0","id":[],"method":"tools/call"}),
    ] {
        let response = discovery_request(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 10000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(body["error"]["code"].is_i64());
    }
}
#[async_trait::async_trait]
impl ChannelToolBackend for DeniedBackend {
    async fn authorize_connection(&self, _: &RuntimeCredential) -> Result<(), OutboxError> {
        self.authenticated.fetch_add(1, Ordering::SeqCst);
        Err(OutboxError::ToolUnavailable)
    }
    async fn authenticate(
        &self,
        _: &RuntimeCredential,
    ) -> Result<ChannelReplySessionAuthorization, OutboxError> {
        panic!("unauthenticated session")
    }
    async fn context(
        &self,
        _: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError> {
        panic!("unauthenticated context")
    }
    async fn reply(
        &self,
        _: &ChannelReplySessionAuthorization,
        _: &str,
        _: ChannelReplyToolRequest,
    ) -> Result<ReplyToolReceipt, OutboxError> {
        panic!("unauthenticated reply")
    }
}

#[tokio::test]
async fn only_distinct_cma_bearer_reaches_authentication_and_denial_never_falls_back() {
    let backend = Arc::new(DeniedBackend::default());
    for authorization in [
        None,
        Some("Bearer nyxid_ag_fixture_only_not_a_real_key"),
        Some("Bearer cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/api/v1/channel-tools")
            .header("content-type", "application/json")
            .header("cookie", "fixture_browser_session=present");
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response=router(backend.clone(),"https://cma.example").oneshot(request.body(axum::body::Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"reply_to_channel","arguments":{"text":"hello"}}}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    assert_eq!(backend.authenticated.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cross_origin_duplicate_authorization_and_invalid_version_reject_before_auth() {
    let backend = Arc::new(DeniedBackend::default());
    for (name, value, status) in [
        ("origin", "https://other.example", StatusCode::FORBIDDEN),
        ("authorization", "Bearer another", StatusCode::BAD_REQUEST),
        (
            "mcp-protocol-version",
            "unsupported",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/channel-tools")
            .header("content-type", "application/json")
            .header(
                "authorization",
                "Bearer cma_ct_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .header(name, value)
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            router(backend.clone(), "https://cma.example")
                .oneshot(request)
                .await
                .unwrap()
                .status(),
            status
        );
    }
    assert_eq!(backend.authenticated.load(Ordering::SeqCst), 0);
}
