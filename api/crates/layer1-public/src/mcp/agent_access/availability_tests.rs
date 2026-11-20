use super::*;
use std::io::Write;
use tracing::{instrument::WithSubscriber, Instrument};

struct UnavailableAuth {
    timeout: bool,
}
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for UnavailableAuth {
    async fn authenticate_setup(&self, _: &str) -> Result<(), AgentAccessError> {
        if self.timeout {
            std::future::pending::<()>().await;
        }
        Err(AgentAccessError::Unavailable)
    }
    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AgentAccessRequester, AgentAccessError> {
        self.authenticate_setup(capability).await?;
        unreachable!()
    }
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
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

async fn check(timeout: bool, rpc: Value, reason: &str) {
    let stub = Stub::new(true);
    let app = agent_access_router(Arc::new(UnavailableAuth { timeout }), stub.clone());
    let logs = Logs::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(logs.clone())
        .finish();
    let response = async {
        let mut request = request(&stub, &rpc);
        request.headers_mut().insert(
            "x-request-id",
            HeaderValue::from_static("private-client-id"),
        );
        app.oneshot(request)
            .instrument(tracing::info_span!(
                "http.request",
                credential = "private-parent-credential"
            ))
            .await
            .unwrap()
    }
    .with_subscriber(subscriber)
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    uuid::Uuid::parse_str(&request_id).unwrap();
    let bytes = to_bytes(response.into_body(), 2048).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"error":"Agent access service is temporarily unavailable"})
    );
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let entries: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let events: Vec<_> = entries
        .iter()
        .filter(|entry| entry["target"] == "cma::agent_access")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fields"]["reason"], reason);
    assert_eq!(events[0]["fields"]["request_id"], request_id);
    assert!(!output.contains(&stub.bearer));
    assert!(!output.contains("private-parent-credential"));
    assert!(!output.contains("private-client-id"));
    assert!(!output.contains("missing-github-1"));
    assert!(stub.proposals.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn accept_timeout_has_a_bounded_reason_and_public_correlation() {
    check(true, call(proposal()), "accept_timeout").await;
}

#[tokio::test]
async fn unavailable_authentication_has_a_distinct_reason_and_the_same_static_public_error() {
    check(false, call(proposal()), "authentication_unavailable").await;
    check(false, rpc("ping", json!({})), "authentication_unavailable").await;
}
