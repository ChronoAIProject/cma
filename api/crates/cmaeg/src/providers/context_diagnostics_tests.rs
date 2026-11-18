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
async fn default_context_reads_preserve_wire_and_diagnostic_baseline() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        capture.lock().unwrap().push(headers);
        async { StatusCode::NOT_FOUND }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let provider = ProviderSlug::new("cma_codex").unwrap();
    let binding = BindingId::new("binding").unwrap();
    let conversation = ConversationId::new("conversation").unwrap();
    let event = EventId::new("event").unwrap();
    let credential = Secret::new("synthetic-credential");
    assert!(client
        .find_event_context(&provider, &binding, &conversation, &event, &credential)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        client
            .find_event_context_with_retry(&provider, &binding, &conversation, &event, &credential)
            .await,
        Ok(None)
    ));
    server.abort();
    let requests = received.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests.iter().all(|headers| !headers.contains_key(HEADER)),
        "diagnostic header must be absent by default"
    );
    let output = logs.text();
    assert!(!output.contains("provider context lookup started"));
    assert!(!output.contains("provider context attempt"));
    assert!(!output.contains("lookup_hash"));
    assert!(output.contains("provider context lookup"));
}

#[tokio::test]
async fn context_gets_propagate_unique_attempts_linked_to_one_lookup() {
    for json in [false, true] {
        for retry in [false, true] {
            lookup_log_privacy(json, retry).await;
        }
    }
}

async fn lookup_log_privacy(json: bool, retry: bool) {
    let logs = Logs::default();
    let builder = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone());
    let subscriber = if json {
        tracing::Dispatch::new(builder.json().finish())
    } else {
        tracing::Dispatch::new(builder.finish())
    };
    let _guard = tracing::dispatcher::set_default(&subscriber);
    let request_hash = "b".repeat(64);
    let ingress = tracing::info_span!("cmaeg.request", request_hash);
    attach_request_hash(&ingress, &request_hash);
    let _entered = ingress.enter();
    let turn = tracing::info_span!(
        "cmaeg.turn",
        event_id = "private-parent-event",
        uri = "/private-parent-path",
        credential = "private-parent-credential"
    );
    let _turn = turn.enter();
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        let mut requests = capture.lock().unwrap();
        requests.push(headers);
        let status = if retry && requests.len() == 1 {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::NOT_FOUND
        };
        async move { status }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut client = client(&origin);
    client.context_diagnostics_enabled = true;
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let provider = ProviderSlug::new("cma_codex").unwrap();
    let binding = BindingId::new("private-binding").unwrap();
    let conversation = ConversationId::new("private-conversation").unwrap();
    let event = EventId::new("private-event").unwrap();
    let credential = Secret::new("private-credential");
    if retry {
        assert!(matches!(
            client
                .find_event_context_with_retry(
                    &provider,
                    &binding,
                    &conversation,
                    &event,
                    &credential
                )
                .await,
            Ok(None)
        ));
    } else {
        assert!(client
            .find_event_context(&provider, &binding, &conversation, &event, &credential)
            .await
            .unwrap()
            .is_none());
    }
    server.abort();
    let requests = received.lock().unwrap();
    assert_eq!(requests.len(), if retry { 2 } else { 1 });
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
        assert!(value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert!(!output.contains(value));
        let hash = crate::telemetry::http::correlation_hash(value.as_bytes());
        let completed = output
            .lines()
            .find(|line| {
                line.contains(&hash) && (line.contains("completed") || line.contains("error"))
            })
            .unwrap();
        if json {
            let record: serde_json::Value = serde_json::from_str(completed).unwrap();
            assert_eq!(record["fields"]["attempt"], i + 1);
        } else {
            assert!(completed.contains(&format!("attempt={}", i + 1)));
        }
        headers.push(value);
    }
    if retry {
        assert_ne!(headers[0], headers[1]);
    }
    let lookups: std::collections::BTreeSet<_> = if json {
        output
            .lines()
            .filter_map(|line| {
                let record: serde_json::Value = serde_json::from_str(line).unwrap();
                record["fields"]["lookup_hash"].as_str().map(str::to_owned)
            })
            .collect()
    } else {
        output
            .split_whitespace()
            .filter_map(|field| field.strip_prefix("lookup_hash=").map(str::to_owned))
            .collect()
    };
    assert_eq!(lookups.len(), 1);
    for private in [
        "private-binding",
        "private-conversation",
        "private-event",
        "private-credential",
        "127.0.0.1",
        "private-parent-event",
        "private-parent-path",
        "private-parent-credential",
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
        let mut attempt = Attempt::new(Some(&"a".repeat(64)), 1);
        attempt.phase(phase);
        tokio::time::advance(Duration::from_millis(3000)).await;
        drop(attempt);
        let output = logs.text();
        let last = output.lines().last().unwrap();
        assert!(last.contains(&format!("phase=\"{phase}\"")));
        assert!(last.contains("outcome=\"cancelled\""));
        assert!(last.contains("attempt_elapsed_ms=3000"));
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
    let mut client = client(&format!("http://{}", listener.local_addr().unwrap()));
    client.context_diagnostics_enabled = true;
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

#[tokio::test]
async fn tool_context_read_does_not_join_custom_diagnostics() {
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = received.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        capture.lock().unwrap().push(headers);
        async { StatusCode::NOT_FOUND }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = client(&format!("http://{}", listener.local_addr().unwrap()));
    client.context_diagnostics_enabled = true;
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let result = client
        .event_context(
            &ProviderSlug::new("cma_codex").unwrap(),
            &BindingId::new("binding").unwrap(),
            &ConversationId::new("conversation").unwrap(),
            &EventId::new("event").unwrap(),
            &Secret::new("synthetic-credential"),
        )
        .await;
    server.abort();
    assert!(matches!(
        result,
        Err(crate::providers::ProviderError::Unavailable(_))
    ));
    let requests = received.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].contains_key(HEADER));
}

#[test]
fn lookup_ignores_untyped_or_invalid_request_hashes() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    for value in ["private-request", &"a".repeat(64)] {
        let parent = tracing::info_span!("cmaeg.request", request_hash = value);
        attach_request_hash(&parent, "private-invalid-hash");
        let _entered = parent.enter();
        assert!(lookup(true).is_some());
    }
    for line in logs.text().lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(record["fields"]["request_hash"].is_null());
        assert_eq!(record["fields"]["lookup_hash"].as_str().unwrap().len(), 64);
        assert!(!line.contains("private-"));
        assert!(!line.contains(&"a".repeat(64)));
    }
}

#[tokio::test]
async fn retry_warning_excludes_private_ancestry_when_info_is_filtered() {
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_max_level(tracing::Level::WARN)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let parent = tracing::error_span!(
        "cmaeg.turn",
        event_id = "private-parent-event",
        uri = "/private-parent-path",
        credential = "private-parent-credential"
    );
    let _entered = parent.enter();
    let result =
        super::super::retry(true, |_| async { Err(ContextFailure::response(400, None)) }).await;
    assert!(result.is_err());
    let output = logs.text();
    assert_eq!(output.lines().count(), 1);
    let record: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(record["fields"]["error_class"], "upstream_refusal");
    assert_eq!(record["fields"]["lookup_hash"].as_str().unwrap().len(), 64);
    for private in [
        "private-parent-event",
        "/private-parent-path",
        "private-parent-credential",
    ] {
        assert!(!output.contains(private), "diagnostics leaked {private}");
    }
}
