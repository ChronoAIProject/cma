use super::*;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    routing::any,
    Router,
};
use serde_json::json;
use std::{collections::VecDeque, sync::Mutex as StdMutex};

type Calls = Arc<StdMutex<Vec<(String, HeaderMap, String)>>>;

pub(crate) struct Server {
    pub(crate) url: reqwest::Url,
    pub(crate) calls: Calls,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct StateData {
    calls: Calls,
    replies: StdMutex<VecDeque<(u16, String)>>,
}
async fn handler(
    State(state): State<Arc<StateData>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, String) {
    state.calls.lock().unwrap().push((
        uri.to_string(),
        headers,
        String::from_utf8(body.to_vec()).unwrap(),
    ));
    let (status, body) = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected exchange");
    if status == 0 {
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    (StatusCode::from_u16(status.max(200)).unwrap(), body)
}
impl Server {
    pub(crate) async fn new(replies: Vec<(u16, String)>) -> Self {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let state = Arc::new(StateData {
            calls: calls.clone(),
            replies: StdMutex::new(replies.into()),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/api/v1/billing/sandboxes/",
            listener.local_addr().unwrap()
        )
        .parse()
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(handler)).with_state(state),
            )
            .await
            .unwrap();
        });
        Self { url, calls, task }
    }
    pub(crate) fn provider(&self) -> Arc<ClientCredentialsProvider> {
        ClientCredentialsProvider::new(
            &self.url,
            "test client +&".into(),
            "test secret +&".into(),
            ProducerScope::Funding,
        )
        .unwrap()
    }
}
pub(crate) fn issued(value: &str, ttl: u64) -> (u16, String) {
    (
        200,
        json!({"access_token": value, "token_type": "Bearer", "expires_in": ttl}).to_string(),
    )
}

#[tokio::test]
async fn first_fetch_caches_and_encodes_the_exact_nyxid_contract() {
    let server = Server::new(vec![issued("first", 3600)]).await;
    let provider = server.provider();
    assert_eq!(provider.token().await.unwrap().bearer(), "first");
    assert_eq!(provider.token().await.unwrap().bearer(), "first");
    let calls = server.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "/oauth/token");
    assert_eq!(
        calls[0].1["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert!(!calls[0].1.contains_key("authorization"));
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(calls[0].2.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(form.len(), 4);
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["scope"], "billing:sandbox:write");
    assert_eq!(form["client_id"], "test client +&");
    assert_eq!(form["client_secret"], "test secret +&");
}

#[tokio::test]
async fn proactive_refresh_uses_eighty_percent_or_five_minute_margin() {
    for (ttl, threshold) in [(3600, 2880), (900, 600), (60, 48)] {
        let server = Server::new(vec![issued("first", ttl), issued("second", ttl)]).await;
        let provider = server.provider();
        provider.token().await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(threshold - 1)).await;
        tokio::time::resume();
        assert_eq!(provider.token().await.unwrap().bearer(), "first");
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(2)).await;
        tokio::time::resume();
        assert_eq!(provider.token().await.unwrap().bearer(), "second");
        assert_eq!(server.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn concurrent_initial_refresh_and_stale_401s_are_single_flight() {
    // NyxID can issue identical JWT bytes within one second: identity must be local.
    let server = Server::new(vec![issued("same", 3600), issued("same", 3600)]).await;
    let provider = server.provider();
    let first = futures_util::future::join_all((0..32).map(|_| provider.token())).await;
    assert!(first.iter().all(Result::is_ok));
    assert_eq!(server.calls.lock().unwrap().len(), 1);
    let old = first[0].as_ref().unwrap();
    let refreshed = futures_util::future::join_all((0..32).map(|_| provider.rejected(old))).await;
    assert!(refreshed.iter().all(Result::is_ok));
    assert_eq!(server.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn endpoint_failures_back_off_and_share_the_failure() {
    let server = Server::new(vec![(503, "untrusted body".into()); 3]).await;
    let provider = server.provider();
    let started = Instant::now();
    let results = futures_util::future::join_all((0..16).map(|_| provider.token())).await;
    assert!(results
        .iter()
        .all(|result| matches!(result, Err(ProducerTokenError::Retryable(_)))));
    assert_eq!(server.calls.lock().unwrap().len(), 3);
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn transient_failure_recovers_within_bounded_attempts() {
    let server = Server::new(vec![
        (429, "".into()),
        (503, "".into()),
        issued("recovered", 3600),
    ])
    .await;
    assert_eq!(
        server.provider().token().await.unwrap().bearer(),
        "recovered"
    );
    assert_eq!(server.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn slow_endpoint_has_a_total_deadline_and_no_unbounded_retry() {
    let server = Server::new(vec![(0, "".into()); 3]).await;
    let started = Instant::now();
    assert!(matches!(
        server.provider().token().await,
        Err(ProducerTokenError::Retryable(_))
    ));
    assert_eq!(server.calls.lock().unwrap().len(), 3);
    assert!(started.elapsed() < Duration::from_secs(8));
}

#[tokio::test]
async fn malformed_rejected_and_oversized_responses_are_sanitized() {
    for reply in [
        (401, "reflected credential".into()),
        (302, "redirect".into()),
        (200, "not json".into()),
        issued("", 3600),
        issued("bad\nheader", 3600),
        issued("value", 0),
        issued("value", u64::MAX),
        (
            200,
            json!({"access_token":"value","token_type":"Basic","expires_in":3600}).to_string(),
        ),
        (200, "x".repeat(32769)),
    ] {
        let server = Server::new(vec![reply]).await;
        let result = server.provider().token().await;
        assert!(matches!(result, Err(ProducerTokenError::Invalid(_))));
        assert_eq!(server.calls.lock().unwrap().len(), 1);
    }
}

#[derive(Clone, Default)]
struct Logs(Arc<StdMutex<Vec<u8>>>);
impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
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
#[tokio::test]
async fn logs_contain_categories_and_expiry_but_no_credentials_or_response_body() {
    use tracing::instrument::WithSubscriber;
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .without_time()
        .with_writer(logs.clone())
        .finish();
    async {
        let server = Server::new(vec![
            issued("private-test-bearer", 3600),
            (401, "private-test-response".into()),
        ])
        .await;
        let provider = server.provider();
        let token = provider.token().await.unwrap();
        let error = provider.rejected(&token).await.err().unwrap();
        tracing::warn!(%error, "funding failure");
    }
    .with_subscriber(subscriber)
    .await;
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for value in [
        "test client",
        "test secret",
        "private-test-bearer",
        "private-test-response",
    ] {
        assert!(!output.contains(value));
    }
    for value in [
        "oauth_client_credentials",
        "expires_at=",
        "success",
        "endpoint_rejected",
        "failure",
    ] {
        assert!(output.contains(value), "missing {value}");
    }
}

#[tokio::test]
async fn expiry_failure_never_reuses_stale_credentials_and_recovers_after_cooldown() {
    let server = Server::new(vec![
        issued("old", 3600),
        (503, String::new()),
        (503, String::new()),
        (503, String::new()),
        issued("new", 3600),
    ])
    .await;
    let provider = server.provider();
    provider.token().await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3601)).await;
    tokio::time::resume();
    assert!(matches!(
        provider.token().await,
        Err(ProducerTokenError::Retryable(_))
    ));
    assert!(provider.token().await.is_err());
    assert_eq!(server.calls.lock().unwrap().len(), 4);
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::time::resume();
    assert_eq!(provider.token().await.unwrap().bearer(), "new");
    assert_eq!(server.calls.lock().unwrap().len(), 5);
}

#[tokio::test]
async fn replicas_have_independent_caches_and_connection_refusal_is_bounded() {
    let server = Server::new(vec![
        issued("replica-one", 3600),
        issued("replica-two", 3600),
    ])
    .await;
    let first = server.provider();
    let second = server.provider();
    assert_eq!(first.token().await.unwrap().bearer(), "replica-one");
    assert_eq!(second.token().await.unwrap().bearer(), "replica-two");
    assert_eq!(server.calls.lock().unwrap().len(), 2);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    drop(listener);
    let unavailable =
        ClientCredentialsProvider::new(&url, "id".into(), "secret".into(), ProducerScope::Funding)
            .unwrap();
    assert!(matches!(
        unavailable.token().await,
        Err(ProducerTokenError::Retryable(_))
    ));
}
