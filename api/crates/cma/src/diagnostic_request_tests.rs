use axum::{body::Body, extract::Request};
use axum::{routing::get, Router};
use http_body_util::BodyExt;
use layer2_core::diagnostic_context::DiagnosticContext;
use tower::ServiceExt;

async fn streaming() -> Body {
    let initial = DiagnosticContext::current();
    Body::from_stream(futures_util::stream::once(async move {
        tokio::task::yield_now().await;
        let polled = DiagnosticContext::current();
        assert_eq!(initial.request_hash, polled.request_hash);
        assert_eq!(initial.route, polled.route);
        Ok::<_, std::convert::Infallible>(format!("{} {}", polled.route, polled.request_hash))
    }))
}
#[tokio::test]
async fn template_hash_and_stream_context_survive_concurrent_requests() {
    let app = crate::with_request_context(Router::new().route("/agents/{id}", get(streaming)));
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..20 {
        let app = app.clone();
        tasks.spawn(async move {
            let response = app
                .oneshot(
                    Request::builder()
                        .uri(format!("/agents/private-{i}?payload=private"))
                        .header("x-request-id", format!("private-correlation-{i}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            String::from_utf8(
                response
                    .into_body()
                    .collect()
                    .await
                    .unwrap()
                    .to_bytes()
                    .to_vec(),
            )
            .unwrap()
        });
    }
    let mut hashes = std::collections::HashSet::new();
    while let Some(result) = tasks.join_next().await {
        let text = result.unwrap();
        assert!(text.starts_with("GET /agents/{id} "));
        assert!(!text.contains("private"));
        let hash = text.split_whitespace().last().unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(hashes.insert(hash.to_owned()));
    }
    assert_eq!(DiagnosticContext::current().worker, "unattributed");
}
#[tokio::test]
async fn cancellation_and_background_scopes_restore_the_outer_context() {
    let context = DiagnosticContext::background("scheduler");
    context
        .scope(async {
            assert!(tokio::time::timeout(
                std::time::Duration::from_millis(1),
                DiagnosticContext::background("metrics").scope(std::future::pending::<()>())
            )
            .await
            .is_err());
            assert_eq!(DiagnosticContext::current().worker, "scheduler");
            let child = tokio::spawn(
                DiagnosticContext::background("recovery_assessment")
                    .scope(async { DiagnosticContext::current().worker }),
            )
            .await
            .unwrap();
            assert_eq!(child, "recovery_assessment");
        })
        .await;
    assert_eq!(DiagnosticContext::current().worker, "unattributed");
}

#[derive(Clone, Default)]
struct LogOutput(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for LogOutput {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogOutput {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[tokio::test]
async fn request_log_uses_the_same_hash_and_never_raw_path_query_or_header() {
    let output = LogOutput::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let outer = tracing::info_span!("private", owner = "private-owner");
    let _entered = outer.enter();
    let app = crate::with_request_context(Router::new().route("/agents/{id}", get(streaming)));
    let request_id = uuid::Uuid::new_v4().to_string();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/agents/private-resource?payload=private-query")
                .header("x-request-id", &request_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["x-request-id"], request_id.as_str());
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains(body.split_whitespace().last().unwrap()),
        "{logs}"
    );
    assert!(logs.contains("GET /agents/{id}"));
    assert!(!logs.contains("private"), "{logs}");
    assert!(!logs.contains(&request_id), "{logs}");
}
