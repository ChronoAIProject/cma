use super::*;
use crate::providers::{ProviderClient, ProviderRegistry};
use axum::{
    http::{HeaderMap, StatusCode},
    Router,
};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, EventId, ProviderSlug},
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
impl Logs {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn client(origin: &str) -> ProviderClient {
    let registry = ProviderRegistry::parse(&serde_json::json!([{
        "slug":"cma_codex", "base_url":origin, "kind":"responses_http", "max_inflight":2,
        "paths":{"agent_card":"/card", "binding":"/bindings/{binding_id}", "conversation":"/bindings/{binding_id}/conversations/{conversation_id}", "responses":"/responses"}
    }]).to_string()).unwrap();
    ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 2).unwrap()
}

#[tokio::test]
async fn context_gets_propagate_unique_attempts_linked_to_one_lookup() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let request_hash = "b".repeat(64);
    let ingress = tracing::info_span!("cmaeg.request", request_hash);
    let _entered = ingress.enter();
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        let mut requests = capture.lock().unwrap();
        requests.push(headers);
        let status = if requests.len() == 1 {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NOT_FOUND
        };
        async move { status }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let client = client(&origin);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(matches!(
        client
            .find_event_context_with_retry(
                &ProviderSlug::new("cma_codex").unwrap(),
                &BindingId::new("private-binding").unwrap(),
                &ConversationId::new("private-conversation").unwrap(),
                &EventId::new("private-event").unwrap(),
                &Secret::new("private-credential")
            )
            .await,
        Ok(None)
    ));
    server.abort();
    let requests = received.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let mut headers = Vec::new();
    let output = logs.text();
    let link = output
        .lines()
        .find(|line| line.contains("provider context lookup started"))
        .unwrap();
    assert!(link.contains(&request_hash));
    for (i, request) in requests.iter().enumerate() {
        let value = request[HEADER].to_str().unwrap();
        assert_eq!(value.len(), 32);
        assert!(value.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!output.contains(value));
        let hash = crate::telemetry::http::correlation_hash(value.as_bytes());
        let completed = output
            .lines()
            .find(|line| {
                line.contains(&hash) && line.contains("outcome=\"completed\"")
                    || line.contains(&hash) && line.contains("outcome=\"error\"")
            })
            .unwrap();
        assert!(completed.contains(&format!("attempt={}", i + 1)));
        headers.push(value);
    }
    assert_ne!(headers[0], headers[1]);
    let lookups: std::collections::BTreeSet<_> = output
        .split_whitespace()
        .filter(|field| field.starts_with("lookup_hash="))
        .collect();
    assert_eq!(lookups.len(), 1);
    for private in [
        "private-binding",
        "private-conversation",
        "private-event",
        "private-credential",
        "127.0.0.1",
    ] {
        assert!(!output.contains(private), "diagnostics leaked {private}");
    }
}

#[tokio::test(start_paused = true)]
async fn cancellation_identifies_the_active_phase_without_inheriting_span_data() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let parent = tracing::info_span!(
        "private",
        uri = "private-path",
        credential = "private-credential"
    );
    let _entered = parent.enter();
    for phase in [
        "credential_tracking",
        "request_setup",
        "response_headers",
        "response_body",
        "response_validation",
    ] {
        let mut attempt = Attempt::new(&"a".repeat(64), 1);
        attempt.phase(phase);
        tokio::time::advance(Duration::from_millis(3000)).await;
        drop(attempt);
        let output = logs.text();
        let last = output.lines().last().unwrap();
        assert!(last.contains(&format!("phase=\"{phase}\"")));
        assert!(last.contains("outcome=\"cancelled\""));
        assert!(last.contains("elapsed_ms=3000"));
        assert!(!output.contains("private-path"));
        assert!(!output.contains("private-credential"));
    }
}

#[tokio::test]
async fn response_body_timeout_is_distinct_from_waiting_for_headers() {
    use axum::body::{Body, Bytes};
    use futures_util::StreamExt;
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let app = Router::new().fallback(|| async {
        Body::from_stream(
            futures_util::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"{")) })
                .chain(futures_util::stream::pending()),
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = client
        .find_event_context(
            &ProviderSlug::new("cma_codex").unwrap(),
            &BindingId::new("binding").unwrap(),
            &ConversationId::new("conversation").unwrap(),
            &EventId::new("event").unwrap(),
            &Secret::new("private-credential"),
        )
        .await;
    server.abort();
    assert!(matches!(
        result,
        Err(crate::providers::ProviderError::Unavailable(_))
    ));
    let output = logs.text();
    let finished = output
        .lines()
        .find(|line| line.contains("outcome=\"error\""))
        .unwrap();
    assert!(finished.contains("phase=\"response_body\""));
    assert!(finished.contains("reason=\"timeout\""));
    assert!(finished.contains("upstream_status=200"));
}
