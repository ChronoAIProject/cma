use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use cma_sdk::{Client, Error};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct Server {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(router: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        base_url: format!("http://{address}"),
        task,
    }
}

#[tokio::test]
async fn reads_the_anonymous_discovery_document() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|headers: HeaderMap| async move {
            assert!(headers.get(header::AUTHORIZATION).is_none());
            axum::Json(json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url)
        .unwrap()
        .bearer_token("discovery-omits-this")
        .unwrap()
        .build()
        .unwrap();

    let document = client.read_conformance().await.unwrap();

    assert_eq!(document.service, "cma");
    assert_eq!(document.status, "rebuilding");
    assert!(document.interfaces.is_empty());
    assert!(format!("{client:?}").contains("base_url"));
}

#[tokio::test]
async fn preserves_a_reverse_proxy_prefix() {
    let router = Router::new().route(
        "/gateway/api/v1/conformance",
        get(|| async {
            axum::Json(json!({
                "service": "cma",
                "status": "ready",
                "interfaces": ["rest"],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(format!("{}/gateway", server.base_url))
        .unwrap()
        .build()
        .unwrap();

    let document = client.read_conformance().await.unwrap();

    assert_eq!(document.interfaces, ["rest"]);
}

#[tokio::test]
async fn decodes_generic_problem_documents() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [
                    (header::CONTENT_TYPE, "application/problem+json"),
                    (
                        header::HeaderName::from_static("x-request-id"),
                        "request-801",
                    ),
                ],
                json!({
                    "status": 503,
                    "detail": "deployment is unavailable",
                    "code": "unavailable",
                })
                .to_string(),
            )
                .into_response()
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url).unwrap().build().unwrap();

    let error = client.read_conformance().await.unwrap_err();

    assert_eq!(
        error,
        Error::Api {
            status: 503,
            code: Some("unavailable".into()),
            message: "deployment is unavailable".into(),
            request_id: Some("request-801".into()),
            retry_after: None,
            extensions: Default::default(),
        }
    );
}

#[tokio::test]
async fn rejects_invalid_success_and_failure_documents() {
    for (status, body) in [
        (StatusCode::OK, json!({"service": "cma", "status": ""})),
        (
            StatusCode::OK,
            json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
                "extra": true,
            }),
        ),
        (
            StatusCode::BAD_GATEWAY,
            json!({"status": 503, "detail": "mismatch"}),
        ),
        (StatusCode::BAD_GATEWAY, json!({"code": "missing_detail"})),
    ] {
        let router = Router::new().route(
            "/api/v1/conformance",
            get(move || async move { (status, axum::Json(body)) }),
        );
        let server = serve(router).await;
        let client = Client::builder(&server.base_url).unwrap().build().unwrap();
        assert!(matches!(
            client.read_conformance().await,
            Err(Error::Protocol { .. })
        ));
    }
}

#[tokio::test]
async fn rejects_non_json_problem_responses() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async { (StatusCode::BAD_GATEWAY, "not json") }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url).unwrap().build().unwrap();

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Protocol { .. })
    ));
}

#[tokio::test]
async fn applies_the_configured_deadline() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            axum::Json(json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url)
        .unwrap()
        .request_timeout(Duration::from_millis(5))
        .unwrap()
        .build()
        .unwrap();

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Transport { .. })
    ));
}

#[test]
fn validates_client_configuration() {
    for invalid in [
        "not a URL",
        "ftp://cma.example",
        "https://user@cma.example",
        "https://cma.example?mode=test",
        "https://cma.example/#fragment",
    ] {
        assert!(matches!(
            Client::builder(invalid),
            Err(Error::Protocol { .. })
        ));
    }

    let builder = Client::builder("https://cma.example").unwrap();
    assert!(format!("{builder:?}").contains("base_url"));
    assert!(matches!(
        builder.request_timeout(Duration::ZERO),
        Err(Error::Protocol { .. })
    ));
}

#[test]
fn bearer_credentials_are_validated_and_redacted() {
    for invalid in ["", "with space", "with\nnewline", "\u{e9}"] {
        assert!(matches!(
            Client::builder("https://example.invalid")
                .unwrap()
                .bearer_token(invalid),
            Err(Error::Validation { .. })
        ));
    }
    let builder = Client::builder("https://example.invalid")
        .unwrap()
        .bearer_token("redacted-fixture")
        .unwrap();
    assert!(!format!("{builder:?}").contains("redacted-fixture"));
    let client = builder.build().unwrap();
    assert!(!format!("{client:?}").contains("redacted-fixture"));
    assert_eq!(format!("{client:?}"), format!("{:?}", client.clone()));
}

#[tokio::test]
async fn preserves_structured_refusal_details_and_retry_delays() {
    for (delay, valid) in [
        ("12".to_string(), true),
        (
            httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(600)),
            true,
        ),
        ("0".into(), false),
        ("-1".into(), false),
        ("+1".into(), false),
        ("not a date".into(), false),
        ("Sun, 06 Nov 1994 08:49:37 GMT".into(), false),
    ] {
        let router = Router::new().route("/api/v1/conformance", get(move || async move {
            (StatusCode::TOO_MANY_REQUESTS, [("retry-after", delay), ("x-request-id", "request-rest-1".to_string())], axum::Json(json!({
                "status":429,"code":"admission_limit","detail":"Too many resources","type":"about:blank","instance":"/command",
                "field":"sandbox_profile","path":"workspace_profiles[0]","reason":"limit","counts":{"current":2,"maximum":2}
            })))
        }));
        let server = serve(router).await;
        let error = Client::builder(&server.base_url)
            .unwrap()
            .build()
            .unwrap()
            .read_conformance()
            .await
            .unwrap_err();
        assert!(error.is_retryable());
        assert!(!error.is_unauthenticated());
        assert_eq!(error.extension("field"), Some("sandbox_profile"));
        assert_eq!(error.extension("counts"), None);
        let Error::Api {
            status,
            code,
            request_id,
            retry_after,
            extensions,
            ..
        } = error
        else {
            panic!("expected API error")
        };
        assert_eq!(status, 429);
        assert_eq!(code.as_deref(), Some("admission_limit"));
        assert_eq!(request_id.as_deref(), Some("request-rest-1"));
        assert_eq!(retry_after.is_some(), valid);
        assert_eq!(extensions["counts"]["maximum"], 2);
        assert!(!extensions.contains_key("type"));
        assert!(!extensions.contains_key("instance"));
    }
}

#[tokio::test]
async fn empty_error_fields_fall_back_without_becoming_machine_codes() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async {
            (
                StatusCode::UNAUTHORIZED,
                [("x-request-id", " ")],
                axum::Json(
                    json!({"status":401,"code":" ","detail":" ","title":"Sign in required"}),
                ),
            )
        }),
    );
    let server = serve(router).await;
    let error = Client::builder(&server.base_url)
        .unwrap()
        .build()
        .unwrap()
        .read_conformance()
        .await
        .unwrap_err();
    assert!(error.is_unauthenticated());
    assert!(!error.is_retryable());
    assert!(
        matches!(error, Error::Api { code: None, request_id: None, ref message, .. } if message == "Sign in required")
    );
    let validation = cma_sdk::IdempotencyKey::new("").unwrap_err();
    assert!(validation.extension("reason").is_none());
    assert!(!validation.is_retryable());
    assert!(!validation.to_string().is_empty());
}

#[tokio::test]
async fn redirects_do_not_forward_credentials_and_status_errors_are_not_retried() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let received = Arc::new(AtomicUsize::new(0));
    let target_count = Arc::clone(&received);
    let target = serve(Router::new().fallback(get(move || {
        target_count.fetch_add(1, Ordering::SeqCst);
        async { StatusCode::NO_CONTENT }
    })))
    .await;
    let location = format!("{}/redirected", target.base_url);
    let attempts = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&attempts);
    let source = serve(Router::new().route(
        "/api/v1/agents/agent-1",
        get(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            let location = location.clone();
            async move {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [("location", location)],
                    axum::Json(json!({"status":307,"detail":"moved"})),
                )
            }
        }),
    ))
    .await;
    let client = Client::builder(&source.base_url)
        .unwrap()
        .bearer_token("redirect-fixture")
        .unwrap()
        .build()
        .unwrap();
    assert!(matches!(
        client.agents().read("agent-1").await,
        Err(Error::Api { status: 307, .. })
    ));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(received.load(Ordering::SeqCst), 0);
    let counted = Arc::clone(&attempts);
    let failure = serve(Router::new().route(
        "/api/v1/agents/agent-1",
        get(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(json!({"status":503,"detail":"unavailable"})),
                )
            }
        }),
    ))
    .await;
    let client = Client::builder(&failure.base_url).unwrap().build().unwrap();
    assert!(
        client
            .agents()
            .read("agent-1")
            .await
            .unwrap_err()
            .is_retryable()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

async fn raw_server(header: &'static str, body: Vec<u8>) -> Server {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let received = socket.read(&mut buffer).await.unwrap();
            assert!(received > 0 && request.len() < 16384);
            request.extend_from_slice(&buffer[..received]);
        }
        if socket.write_all(header.as_bytes()).await.is_ok() {
            let _ = socket.write_all(&body).await;
        }
    });
    Server {
        base_url: format!("http://{address}"),
        task,
    }
}

#[tokio::test]
async fn bounds_responses_with_and_without_declared_content_length() {
    let declared = raw_server(
        "HTTP/1.1 200 OK\r\nContent-Length: 8388609\r\nConnection: close\r\n\r\n",
        Vec::new(),
    )
    .await;
    let client = Client::builder(&declared.base_url)
        .unwrap()
        .build()
        .unwrap();
    assert!(
        matches!(client.read_conformance().await, Err(Error::Protocol { message }) if message.contains("8388608-byte"))
    );
    let chunk = vec![b' '; 1024 * 1024];
    let mut body = Vec::new();
    for _ in 0..9 {
        body.extend_from_slice(b"100000\r\n");
        body.extend_from_slice(&chunk);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(b"0\r\n\r\n");
    let chunked = raw_server(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        body,
    )
    .await;
    let client = Client::builder(&chunked.base_url).unwrap().build().unwrap();
    assert!(
        matches!(client.read_conformance().await, Err(Error::Protocol { message }) if message.contains("8388608-byte"))
    );
}

#[tokio::test]
async fn truncated_bodies_are_transport_errors_and_empty_commands_allow_whitespace() {
    let truncated = raw_server(
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
        b"{}".to_vec(),
    )
    .await;
    let client = Client::builder(&truncated.base_url)
        .unwrap()
        .build()
        .unwrap();
    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Transport { .. })
    ));
    let empty = raw_server(
        "HTTP/1.1 202 Accepted\r\nContent-Length: 3\r\nConnection: close\r\n\r\n",
        b" \r\n".to_vec(),
    )
    .await;
    let client = Client::builder(&empty.base_url).unwrap().build().unwrap();
    client
        .agents()
        .interrupt(
            "agent-1",
            &cma_sdk::IdempotencyKey::new("interrupt-1").unwrap(),
        )
        .await
        .unwrap();
}
