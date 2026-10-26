//! A panicking handler is answered with a problem inside its request span.
use axum::{routing::get, Router};
use serde_json::Value;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

const PAYLOAD: &str = "PANIC-PAYLOAD-SHOULD-NOT-LOG";
const PATH_VALUE: &str = "raw-path-value";

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

async fn boom() -> &'static str {
    panic!("{PAYLOAD}")
}

/// The server runs on this current-thread runtime, so a thread-scoped
/// subscriber captures its logs without touching any global state.
#[tokio::test(flavor = "current_thread")]
async fn a_handler_panic_answers_a_500_problem_and_the_router_keeps_serving() {
    let capture = Capture::default();
    let writer = capture.clone();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish(),
    );
    let router = cmaeg::telemetry::http::instrument(
        Router::new()
            .route("/boom/{id}", get(boom))
            .route("/healthy", get(|| async { "ok" })),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{base}/boom/{PATH_VALUE}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let headers = response.headers();
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(headers["cmaeg-protocol-version"], "1");
    assert_eq!(headers["cache-control"], "no-store");
    assert!(headers.contains_key("x-request-id"));
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["code"], "internal");
    assert_eq!(problem["status"], 500);
    assert_eq!(problem["detail"], "internal_error");
    assert!(!problem.to_string().contains(PAYLOAD));

    let healthy = client.get(format!("{base}/healthy")).send().await.unwrap();
    assert_eq!(healthy.status(), 200);
    assert_eq!(healthy.text().await.unwrap(), "ok");
    server.abort();

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let panicked = logs
        .lines()
        .find(|line| line.contains("request handler panicked"))
        .unwrap_or_else(|| panic!("no panic log: {logs}"));
    // The route template identifies the request; the raw path never appears.
    assert!(panicked.contains("cmaeg.request{"), "{logs}");
    assert!(panicked.contains(r#"route="/boom/{id}""#), "{logs}");
    assert!(
        logs.lines()
            .any(|line| line.contains(r#"route="/boom/{id}""#)
                && line.contains("request finished status=500")),
        "{logs}"
    );
    assert!(!logs.contains(PAYLOAD), "{logs}");
    assert!(!logs.contains(PATH_VALUE), "{logs}");
}
