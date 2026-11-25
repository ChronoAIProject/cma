//! Exercise the real HTTP transport against a bounded local fake NyxID.
use super::*;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::any,
    Router,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
pub(super) struct Reply {
    status: u16,
    body: String,
    pub(super) retry_after: Option<String>,
}
#[derive(Default)]
pub(super) struct HttpState {
    replies: Mutex<VecDeque<Reply>>,
    pub(super) calls: Mutex<Vec<(Method, String, HeaderMap, Value)>>,
}
pub(super) struct Server {
    pub(super) state: Arc<HttpState>,
    pub(super) provider: Arc<NyxIdFundingProvider>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn handler(
    State(state): State<Arc<HttpState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    state.calls.lock().unwrap().push((
        method,
        uri.path().into(),
        headers,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    ));
    let reply = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected provider call");
    let body = if reply.body == "$quote" {
        let calls = state.calls.lock().unwrap();
        let payload = &calls.last().unwrap().3;
        json!({"quote_id":"8e660487-dba4-4be9-ae06-4593b41c4a30","owner_id":payload["owner_id"],"sandbox_id":payload["sandbox_id"],"maximum_allocation":payload["allocation"],"accept_before":"2026-11-05T00:00:00Z","rate_card_version":1}).to_string()
    } else if reply.body == "$window" {
        let calls = state.calls.lock().unwrap();
        let payload = &calls
            .iter()
            .rev()
            .find(|call| call.0 == Method::POST && call.1.ends_with("/windows"))
            .unwrap()
            .3;
        let mut value = payload.clone();
        value["window_id"] = json!("9e660487-dba4-4be9-ae06-4593b41c4a30");
        value["covered_from"] = json!(100);
        value["covered_until"] = json!(400);
        value["components"] = json!([]);
        value.to_string()
    } else {
        reply.body
    };
    if reply.status == 0 {
        // Commit was observed by the fake server, but its response misses the client timeout.
        tokio::time::sleep(Duration::from_secs(6)).await;
    }
    let mut headers = HeaderMap::new();
    if let Some(value) = reply.retry_after {
        headers.insert("retry-after", value.parse().unwrap());
    }
    (
        StatusCode::from_u16(reply.status.max(200)).unwrap(),
        headers,
        body,
    )
}
impl Server {
    pub(super) async fn new(replies: Vec<Reply>) -> Self {
        let state = Arc::new(HttpState {
            replies: Mutex::new(replies.into()),
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new()
            .fallback(any(handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        // Use a harmless existing environment reference; tests never mutate process secrets.
        let provider = NyxIdFundingProvider::new(SandboxBillingConfig {
            base_url: format!("http://{addr}/api/v1/billing/sandboxes/")
                .parse()
                .unwrap(),
            credential_ref: "USER".into(),
            client_credentials: None,
            producer_id: uuid::Uuid::new_v4().to_string(),
        })
        .unwrap();
        Self {
            state,
            provider,
            task,
        }
    }
}
pub(super) fn reply(status: u16, body: Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
        retry_after: None,
    }
}
fn window_json(request: &ReserveRequest) -> Value {
    json!({"request_id":request.request_id,"quote_id":request.quote_id,"window_id":"8e660487-dba4-4be9-ae06-4593b41c4a30","owner_id":request.owner_id,"sandbox_id":request.sandbox_id,"runtime_generation":request.runtime_generation,"covered_from":100,"covered_until":400,"allocation":{"cpu_millis":1000,"memory_mib":2048,"disk_gib":64},"components":[],"evidence_accepted":false,"last_evidence_revision":null})
}
fn request() -> ReserveRequest {
    let s = sandbox();
    ReserveRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        quote_id: uuid::Uuid::new_v4().to_string(),
        owner_id: s.owner.to_string(),
        sandbox_id: s.id.to_string(),
        runtime_generation: 1,
        operation_id: uuid::Uuid::new_v4().to_string(),
        previous_window_id: None,
        seconds: 300,
        allocation: maximum_allocation(&s),
    }
}

#[tokio::test]
async fn real_transport_sends_contract_fields_and_retains_full_receipts() {
    let request = request();
    let window = window_json(&request);
    let quote = json!({"quote_id":request.quote_id,"owner_id":request.owner_id,"sandbox_id":request.sandbox_id,"maximum_allocation":window["allocation"],"accept_before":"2026-11-05T00:00:00Z","rate_card_version":7,"credits_per_unit":{"cpu_millis_seconds":"0.000000010000"}});
    let mut ack = window.clone();
    ack["evidence_accepted"] = json!(true);
    let server = Server::new(vec![
        reply(200, quote.clone()),
        reply(200, window.clone()),
        reply(200, window.clone()),
        reply(200, ack),
    ])
    .await;
    let q = server
        .provider
        .quote(QuoteRequest {
            owner_id: request.owner_id.clone(),
            sandbox_id: request.sandbox_id.clone(),
            allocation: request.allocation.clone(),
        })
        .await
        .unwrap();
    assert_eq!(q.receipt, quote);
    let w = server.provider.reserve(request.clone()).await.unwrap();
    assert_eq!(w.receipt, window);
    assert_eq!(
        server.provider.recover(&request.request_id).await.unwrap(),
        Some(w.clone())
    );
    let e = event(&sandbox(), 1, 100, 400);
    server
        .provider
        .settle(EvidenceRequest {
            window_id: w.window_id,
            owner_id: request.owner_id.clone(),
            sandbox_id: request.sandbox_id.clone(),
            runtime_generation: 1,
            intervals: vec![EvidenceInterval {
                revision: e.revision,
                started_at: e.started_at,
                ended_at: e.ended_at,
                allocation: e.allocation,
                quantity: e.quantity,
            }],
        })
        .await
        .unwrap();
    let calls = server.state.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].1, "/api/v1/billing/sandboxes/quotes");
    assert!(calls[0].2.contains_key("authorization"));
    assert!(!calls[0].2.contains_key("x-nyxid-producer-id"));
    assert!(calls[1].3["allocation"].get("running").is_none());
    assert_eq!(calls[1].3.as_object().unwrap().len(), 9);
    assert!(calls[3].3.get("window_id").is_none());
    assert!(calls[3].3["intervals"][0]["quantity"]
        .get("uptime_seconds")
        .is_none());
}
#[tokio::test]
async fn errors_never_echo_upstream_text_and_only_404_is_an_absent_receipt() {
    let statuses = [404, 402, 409, 400, 401, 403, 413, 415, 422, 500, 502, 302];
    let server = Server::new(
        statuses
            .iter()
            .map(|&status| Reply {
                status,
                body: "SECRET ECHO invalid upstream body".into(),
                retry_after: None,
            })
            .collect(),
    )
    .await;
    for status in statuses {
        let result = server.provider.recover("request").await;
        if status == 404 {
            assert_eq!(result.unwrap(), None);
            continue;
        }
        let error = result.unwrap_err();
        assert!(!error.to_string().contains("SECRET"));
        match status {
            402 | 409 => assert!(matches!(error, FundingError::Unfunded(_))),
            400 | 401 | 403 | 413 | 415 | 422 => assert!(matches!(error, FundingError::Invalid(_))),
            _ => assert!(matches!(error, FundingError::Retryable(_))),
        }
    }
}
#[tokio::test]
async fn retry_after_seconds_and_http_date_are_honored() {
    let later = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
    let server = Server::new(vec![
        Reply {
            status: 429,
            body: String::new(),
            retry_after: Some("90".into()),
        },
        Reply {
            status: 429,
            body: String::new(),
            retry_after: Some(later),
        },
        Reply {
            status: 429,
            body: String::new(),
            retry_after: None,
        },
    ])
    .await;
    assert_eq!(
        server.provider.recover("r").await,
        Err(FundingError::RateLimited(90))
    );
    assert!(matches!(
        server.provider.recover("r").await,
        Err(FundingError::RateLimited(118..=120))
    ));
    assert_eq!(
        server.provider.recover("r").await,
        Err(FundingError::RateLimited(30))
    );
}
#[tokio::test]
async fn malformed_and_oversized_success_responses_cannot_confirm_coverage() {
    let request = request();
    let mut malformed = window_json(&request);
    malformed["allocation"]["cpu_millis"] = json!(u64::MAX);
    let server = Server::new(vec![
        reply(200, json!({})),
        reply(200, malformed),
        Reply {
            status: 200,
            body: "not json".into(),
            retry_after: None,
        },
        Reply {
            status: 200,
            body: "x".repeat(MAX_PROVIDER_RESPONSE_BYTES + 1),
            retry_after: None,
        },
    ])
    .await;
    for _ in 0..4 {
        assert!(matches!(
            server.provider.recover("r").await,
            Err(FundingError::Invalid(_))
        ));
    }
}
#[tokio::test]
async fn evidence_acknowledgement_must_match_exact_window() {
    let request = request();
    let server = Server::new(vec![reply(200, window_json(&request))]).await;
    assert!(server
        .provider
        .settle(EvidenceRequest {
            window_id: "wrong".into(),
            owner_id: request.owner_id,
            sandbox_id: request.sandbox_id,
            runtime_generation: 1,
            intervals: vec![]
        })
        .await
        .is_err());
}
#[tokio::test]
async fn unavailable_credential_and_connection_failure_are_closed() {
    let provider = NyxIdFundingProvider::new(SandboxBillingConfig {
        base_url: "http://127.0.0.1:1/".parse().unwrap(),
        credential_ref: "CMA_TEST_UNSET".into(),
        client_credentials: None,
        producer_id: uuid::Uuid::new_v4().to_string(),
    })
    .unwrap();
    assert!(matches!(
        provider.recover("r").await,
        Err(FundingError::Invalid(_))
    ));
    let provider = NyxIdFundingProvider::new(SandboxBillingConfig {
        base_url: "http://127.0.0.1:1/".parse().unwrap(),
        credential_ref: "USER".into(),
        client_credentials: None,
        producer_id: uuid::Uuid::new_v4().to_string(),
    })
    .unwrap();
    assert!(matches!(
        provider.recover("r").await,
        Err(FundingError::Retryable(_))
    ));
}

#[tokio::test]
async fn adapter_recovers_a_committed_http_timeout_from_durable_request_after_restart() {
    let server = Server::new(vec![
        Reply {
            status: 200,
            body: "$quote".into(),
            retry_after: None,
        },
        reply(404, json!({"error":"not_found"})),
        Reply {
            status: 0,
            body: "$window".into(),
            retry_after: None,
        },
        Reply {
            status: 200,
            body: "$window".into(),
            retry_after: None,
        },
    ])
    .await;
    let state = Arc::new(MemoryState::default());
    let control = Arc::new(Control::default());
    let sandbox = sandbox();
    let adapter =
        ProviderFundingAdapter::new(server.provider.clone(), state.clone(), control.clone());
    assert!(matches!(
        adapter.ensure_coverage(&sandbox, at(100)).await,
        Err(FundingError::Retryable(_))
    ));
    let persisted = state.load(&sandbox.id).await.unwrap().unwrap();
    assert!(persisted.current.is_none());
    assert!(persisted.pending.is_some());
    assert!(control.deadlines.lock().unwrap().is_empty());
    let restarted =
        ProviderFundingAdapter::new(server.provider.clone(), state.clone(), control.clone());
    assert_eq!(
        restarted.ensure_coverage(&sandbox, at(110)).await.unwrap(),
        Some(400)
    );
    let recovered = state.load(&sandbox.id).await.unwrap().unwrap();
    assert!(recovered.pending.is_none());
    assert_eq!(recovered.windows.len(), 1);
    let calls = server.state.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.0 == Method::POST && call.1.ends_with("/windows"))
            .count(),
        1
    );
    assert!(calls
        .last()
        .unwrap()
        .1
        .ends_with(&persisted.pending.unwrap().request_id));
}

#[tokio::test]
async fn provider_outage_402_is_retryable_but_insufficient_credit_remains_unfunded() {
    let server = Server::new(vec![
        reply(
            402,
            json!({"error":"billing_provider_unavailable","message":"SECRET"}),
        ),
        reply(
            402,
            json!({"error":"billing_not_configured","message":"SECRET"}),
        ),
        reply(
            402,
            json!({"error":"insufficient_credits","message":"SECRET"}),
        ),
    ])
    .await;
    for _ in 0..2 {
        let error = server.provider.recover("r").await.unwrap_err();
        assert!(matches!(error, FundingError::Retryable(_)));
        assert!(!error.to_string().contains("SECRET"));
    }
    assert!(matches!(
        server.provider.recover("r").await,
        Err(FundingError::Unfunded(_))
    ));
}
