use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

const SECRET: &str = "issuer-secret";
fn capability() -> String {
    format!("cma_pt_{}_{}", "a".repeat(64), "b".repeat(64))
}
struct Source {
    upstream: url::Url,
    confirmed: bool,
}
#[async_trait::async_trait]
impl Authenticate for Source {
    async fn current(
        &self,
        value: &str,
        setup: bool,
    ) -> layer3_adapter::Result<(url::Url, SecretValue)> {
        if value != capability() || !setup && !self.confirmed {
            return Err(layer3_adapter::AdapterError::Rejected(
                "test refusal".into(),
            ));
        }
        Ok((self.upstream.clone(), SecretValue::new(SECRET.into())))
    }
}

async fn remote(mode: u8) -> (url::Url, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let trap = calls.clone();
    let app = Router::new()
        .route(
            "/mcp",
            post(move |request: Request| {
                let calls = observed.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(
                        request.headers().get(header::AUTHORIZATION).unwrap(),
                        "Bearer issuer-secret"
                    );
                    for header in [
                        "origin",
                        "cookie",
                        "proxy-authorization",
                        "x-forwarded-host",
                        "x-destination",
                    ] {
                        assert!(!request.headers().contains_key(header));
                    }
                    match mode {
                        1 => (
                            StatusCode::TEMPORARY_REDIRECT,
                            [(header::LOCATION, "/trap")],
                            "redirect",
                        )
                            .into_response(),
                        2 => (
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"result":"\u0069ssuer-secret"}"#,
                        )
                            .into_response(),
                        3 => Response::new(Body::from(vec![b'x'; RESPONSE_BYTES + 1])),
                        4 => ([(header::CONTENT_ENCODING, "gzip")], "{}").into_response(),
                        _ => (
                            [(header::CONTENT_TYPE, "application/json")],
                            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
                        )
                            .into_response(),
                    }
                }
            }),
        )
        .route(
            "/trap",
            post(move || {
                let trap = trap.clone();
                async move {
                    trap.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
        url::Url::parse(&format!("http://{}/mcp", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (endpoint, calls, task)
}

fn request(body: impl Into<Body>) -> Request {
    Request::builder()
        .method("POST")
        .uri("/api/v1/provider-tools")
        .header(header::AUTHORIZATION, format!("Bearer {}", capability()))
        .header(header::CONTENT_TYPE, "application/json")
        .header("origin", "https://evil.example")
        .header("cookie", "ambient-cookie")
        .header("proxy-authorization", "another-secret")
        .header("x-destination", "https://evil.example")
        .header("x-forwarded-host", "evil.example")
        .body(body.into())
        .unwrap()
}

#[tokio::test]
async fn relay_uses_only_fixed_custody_and_pending_setup_has_no_tool_effect() {
    let (upstream, calls, task) = remote(0).await;
    let app = routes(Arc::new(Source {
        upstream,
        confirmed: false,
    }));
    let response = app
        .clone()
        .oneshot(request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(to_bytes(response.into_body(), 1024)
        .await
        .unwrap()
        .windows(4)
        .any(|v| v == b"true"));
    let response = app
        .oneshot(request(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"send"}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn relay_refuses_redirect_encoded_or_oversized_responses_and_escaped_credentials() {
    for mode in [1, 2, 3, 4] {
        let (upstream, calls, task) = remote(mode).await;
        let app = routes(Arc::new(Source {
            upstream,
            confirmed: true,
        }));
        let response = app
            .oneshot(request(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"one-effect"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "mode {mode}");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(!contains(&body, SECRET.as_bytes()));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "uncertain calls must not retry"
        );
        task.abort();
    }
}

#[tokio::test]
async fn relay_rejects_bad_protocol_batches_destination_overrides_and_large_requests() {
    let (upstream, calls, task) = remote(0).await;
    let app = routes(Arc::new(Source {
        upstream,
        confirmed: true,
    }));
    for body in [
        r#"[{"jsonrpc":"2.0","method":"tools/list"}]"#,
        r#"{"jsonrpc":"2.0","method":"initialize","method":"tools/call"}"#,
        r#"{"jsonrpc":"2.0","method":"arbitrary"}"#,
    ] {
        assert_eq!(
            app.clone().oneshot(request(body)).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut override_request = request("{}");
    *override_request.uri_mut() = "/api/v1/provider-tools?destination=https://evil.example"
        .parse()
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(override_request)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let response = app
        .oneshot(request(vec![b'x'; REQUEST_BYTES + 1]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    task.abort();
}
#[tokio::test]
async fn relay_account_deletion_is_a_typed_forbidden_problem() {
    struct Erasing;
    #[async_trait::async_trait]
    impl Authenticate for Erasing {
        async fn current(
            &self,
            _: &str,
            _: bool,
        ) -> layer3_adapter::Result<(url::Url, SecretValue)> {
            Err(layer3_adapter::AdapterError::AccessRevoked.context("current relay authority"))
        }
    }
    let response = routes(Arc::new(Erasing))
        .oneshot(request(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let value: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(value["code"], "account_deletion_in_progress");
}
