//! Exercise the emitted completion fields, including extractor and unclassified failures.
use super::*;
use crate::{auth::CreatorAuthError, http::problems::problem};
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use cmaeg_core::problem::ProblemCode;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn completion_fields_are_safe_for_channels_native_lark_and_other_5xx() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let router = instrument(
        Router::new()
            .route(
                "/v1/channels/{channel_id}",
                get(|| async { CreatorAuthError::ProviderUnavailable.into_response() }),
            )
            .route(
                "/v1/native-lark/{channel_id}",
                post(|| async {
                    FailureStage::NativeLarkSource.response(problem(
                        StatusCode::SERVICE_UNAVAILABLE,
                        ProblemCode::ProviderUnavailable,
                        "private-detail-token",
                    ))
                }),
            )
            .route("/other/{id}", get(|| async { StatusCode::BAD_GATEWAY }))
            .route("/success", get(|| async { StatusCode::OK })),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();
    for (method, path, expected_status) in [
        (reqwest::Method::GET, "/v1/channels/private%2Dchannel", 503),
        (reqwest::Method::HEAD, "/v1/channels/private%2Dchannel", 503),
        (
            reqwest::Method::POST,
            "/v1/native-lark/private%2Dchannel",
            503,
        ),
        (reqwest::Method::GET, "/other/private-path", 502),
        (reqwest::Method::GET, "/success", 200),
    ] {
        let response = client
            .request(
                method,
                format!("http://{address}{path}?token=private-query"),
            )
            .header("x-request-id", "private-request-id")
            .bearer_auth("private-bearer")
            .body("private-body")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected_status);
        assert_eq!(response.headers()["x-request-id"], "private-request-id");
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    for private in [
        "private-channel",
        "private%2Dchannel",
        "private-path",
        "private-request-id",
        "private-query",
        "private-bearer",
        "private-body",
        "private-detail-token",
        &address.to_string(),
    ] {
        assert!(
            !logs.contains(private),
            "private value in completion telemetry"
        );
    }
    let completed: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("request finished"))
        .collect();
    assert_eq!(completed.len(), 5, "{logs}");
    for (line, method, stage, code) in [
        (
            completed[0],
            "GET",
            "creator_authentication",
            "provider_unavailable",
        ),
        (
            completed[1],
            "HEAD",
            "creator_authentication",
            "provider_unavailable",
        ),
        (
            completed[2],
            "POST",
            "native_lark_source",
            "provider_unavailable",
        ),
        (completed[3], "GET", "unclassified", "unclassified"),
        (completed[4], "GET", "none", "none"),
    ] {
        assert!(line.contains(&format!("method=\"{method}\"")), "{line}");
        assert!(
            line.contains(&format!("failure_stage=\"{stage}\"")),
            "{line}"
        );
        assert!(line.contains(&format!("problem_code=\"{code}\"")), "{line}");
        assert!(
            line.contains(&correlation_hash(b"private-request-id")),
            "{line}"
        );
        let elapsed: f64 = line
            .split("elapsed_ms=")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(elapsed.is_finite() && elapsed >= 0.0);
    }
    for line in &completed[..3] {
        assert!(
            line.contains(&correlation_hash(b"private-channel")),
            "{line}"
        );
    }
    for line in &completed[3..] {
        assert!(!line.contains("channel_hash="), "{line}");
    }
}
