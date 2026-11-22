use super::*;
use axum::body::Body;
use axum::http::Response;

const SCOPE_BODY: &str = r#"{"error":"api_key_scope_forbidden","error_code":9000,"message":"PRIVATE_MESSAGE_SENTINEL","details":{"token":"PRIVATE_TOKEN_SENTINEL"}}"#;

async fn check_response(
    stage: &'static str,
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    hint: &str,
) {
    check_body(
        stage,
        status,
        content_type,
        move || Body::from(body.clone()),
        hint,
    )
    .await;
}

async fn check_body(
    stage: &'static str,
    status: u16,
    content_type: &'static str,
    body: impl Fn() -> Body + Clone + Send + Sync + 'static,
    hint: &str,
) {
    let mut f = Fixture::new();
    f.serve_response(Some(stage), move || {
        Response::builder()
            .status(status)
            .header("content-type", content_type)
            .header("x-private", "PRIVATE_HEADER_SENTINEL")
            .body(body())
            .unwrap()
    });
    let log = Log::default();
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        prepare(&f.config, &f.config.home, &f.request).with_subscriber(log.subscriber()),
    )
    .await
    .expect("error-body diagnostics cannot consume the repository request budget")
    .unwrap_err();
    let (code, detail, retryable, class) = if matches!(status, 401 | 403) {
        (
            "repository_connection_required",
            "GitHub authorization is missing or does not allow this repository",
            false,
            "http_refused",
        )
    } else {
        (
            "repository_source_unavailable",
            "GitHub source preparation is temporarily unavailable",
            true,
            "http_unavailable",
        )
    };
    assert_eq!(
        (error.code.as_str(), error.detail.as_str(), error.retryable),
        (code, detail, retryable)
    );
    log.failure(stage, Some(status), class, true);
    assert!(
        log.text().contains(&format!("response_hint=\"{hint}\"")),
        "{}",
        log.text()
    );
    let expected = match stage {
        "metadata" => vec!["metadata"],
        "commits" => vec!["metadata", "commits"],
        _ => vec!["metadata", "commits", "archive"],
    };
    assert_eq!(*f.reads.lock().unwrap(), expected);
    let path = f.config.home.join("checkout");
    if stage == "archive" {
        assert_eq!(
            std::fs::read_to_string(path.join(".cma-source-pin")).unwrap(),
            "a".repeat(40)
        );
        assert!(!path.join(".git").exists());
    } else {
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn exact_scope_response_claim_is_bounded_diagnostics_at_every_repository_stage() {
    for stage in ["metadata", "commits", "archive"] {
        check_response(
            stage,
            403,
            "application/json; charset=utf-8",
            SCOPE_BODY.as_bytes().to_vec(),
            "claimed_api_key_scope_forbidden",
        )
        .await;
    }
}

#[tokio::test]
async fn other_error_responses_remain_unknown_without_changing_disposition_or_leaking_body() {
    let cases = [
        (401, "application/json", SCOPE_BODY),
        (
            400,
            "application/json",
            r#"{"error":"bad_request","error_code":1000,"message":"Provider connection required PRIVATE_MESSAGE_SENTINEL"}"#,
        ),
        (
            429,
            "application/json",
            r#"{"error":"rate_limited","error_code":1005}"#,
        ),
        (503, "text/html", "<html>PRIVATE_ERROR_SENTINEL</html>"),
        (
            403,
            "application/json",
            r#"{"message":"API rate limit exceeded PRIVATE_MESSAGE_SENTINEL"}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"api_key_scope_forbidden"}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"PRIVATE_UNKNOWN_ERROR_SENTINEL","error_code":9000}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"api_key_scope_forbidden","error_code":9001}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"api_key_scope_forbidden","error_code":"9000"}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"api_key_scope_forbidden","error_code":9000.0}"#,
        ),
        (
            403,
            "application/json",
            r#"{"error":"api_key_scope_forbidden","error":"PRIVATE_DUPLICATE_SENTINEL","error_code":9000}"#,
        ),
        (403, "application/json", "{PRIVATE_MALFORMED_SENTINEL"),
        (403, "text/html", SCOPE_BODY),
        (403, "", SCOPE_BODY),
    ];
    for stage in ["metadata", "commits", "archive"] {
        for (status, content_type, body) in cases {
            check_response(
                stage,
                status,
                content_type,
                body.as_bytes().to_vec(),
                "unknown",
            )
            .await;
        }
    }
}

#[tokio::test]
async fn diagnostic_json_is_accepted_only_within_the_byte_limit() {
    for (length, expected) in [(8192, "claimed_api_key_scope_forbidden"), (8193, "unknown")] {
        let mut body = SCOPE_BODY.as_bytes().to_vec();
        body.resize(length, b' ');
        check_response("metadata", 403, "application/json", body, expected).await;
    }
}

#[tokio::test]
async fn chunked_error_responses_cannot_escape_the_same_byte_limit() {
    for (length, expected) in [(8192, "claimed_api_key_scope_forbidden"), (8193, "unknown")] {
        let mut bytes = SCOPE_BODY.as_bytes().to_vec();
        bytes.resize(length, b' ');
        let chunks: Vec<_> = bytes
            .chunks(1024)
            .map(axum::body::Bytes::copy_from_slice)
            .collect();
        check_body(
            "metadata",
            403,
            "application/json",
            move || {
                Body::from_stream(futures_util::stream::iter(
                    chunks.clone().into_iter().map(Ok::<_, std::io::Error>),
                ))
            },
            expected,
        )
        .await;
    }
}

fn stalled_body() -> Body {
    Body::from_stream(futures_util::stream::pending::<
        std::result::Result<axum::body::Bytes, std::io::Error>,
    >())
}

#[tokio::test]
async fn stalled_error_body_keeps_the_original_terminal_failure_and_source_pin() {
    for stage in ["metadata", "commits", "archive"] {
        check_body(stage, 403, "application/json", stalled_body, "unknown").await;
    }
}

async fn stalled_response(timeout: Duration) -> (Fixture, reqwest::Response, tokio::time::Instant) {
    let mut f = Fixture::new();
    f.serve_response(Some("metadata"), || {
        Response::builder()
            .status(403)
            .header("content-type", "application/json")
            .body(stalled_body())
            .unwrap()
    });
    let client = reqwest::Client::builder()
        .unix_socket(f.config.github_proxy.as_ref().unwrap().socket())
        .no_proxy()
        .timeout(timeout)
        .build()
        .unwrap();
    let started = tokio::time::Instant::now();
    let response = client
        .get("http://api.github.com/repos/synthetic/source")
        .send()
        .await
        .unwrap();
    (f, response, started)
}

#[tokio::test]
async fn diagnostic_body_wait_is_capped_at_one_hundred_milliseconds() {
    let (_fixture, response, _) = stalled_response(Duration::from_secs(120)).await;
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let hint = crate::repository_snapshot::error_diagnostics::read_hint(response).await;
    assert_eq!(hint.as_str(), "unknown");
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(
        started.elapsed() <= Duration::from_millis(101),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn diagnostic_read_does_not_extend_the_original_request_deadline() {
    let (_fixture, response, request_started) = stalled_response(Duration::from_secs(2)).await;
    tokio::time::pause();
    let near_deadline = request_started + Duration::from_millis(1980);
    tokio::time::advance(near_deadline.saturating_duration_since(tokio::time::Instant::now()))
        .await;
    let started = tokio::time::Instant::now();
    let hint = crate::repository_snapshot::error_diagnostics::read_hint(response).await;
    assert_eq!(hint.as_str(), "unknown");
    assert!(
        started.elapsed() <= Duration::from_millis(25),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn truncated_error_body_cannot_change_the_received_refusal_into_a_transport_failure() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut f = Fixture::new();
    let listener = f.proxy();
    let reads = f.reads.clone();
    f.server = Some(tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        reads.lock().unwrap().push("metadata".into());
        socket.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n").await.unwrap();
        socket.write_all(SCOPE_BODY.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    }));
    let log = Log::default();
    let error = prepare(&f.config, &f.config.home, &f.request)
        .with_subscriber(log.subscriber())
        .await
        .unwrap_err();
    assert_eq!(error.code, "repository_connection_required");
    assert_eq!(
        error.detail,
        "GitHub authorization is missing or does not allow this repository"
    );
    assert!(!error.retryable);
    log.failure("metadata", Some(403), "http_refused", true);
    assert!(log.text().contains("response_hint=\"unknown\""));
    assert_eq!(*f.reads.lock().unwrap(), ["metadata"]);
    assert!(!f.config.home.join("checkout").exists());
}
