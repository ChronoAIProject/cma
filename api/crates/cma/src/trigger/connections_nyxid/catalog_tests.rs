use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    routing::get,
    Router,
};
use layer2_core::trigger_domain::{ServiceConnectionBroker, ServiceConnectionMode};
use tokio::sync::Semaphore;

use super::super::{SecretValue, StoreError, VisitorCredentialSource};
use super::*;

struct Gate {
    credentials_started: AtomicUsize,
    credentials_release: Semaphore,
    reads_started: AtomicUsize,
    reads_release: Semaphore,
    catalog_body: &'static str,
    services_status: StatusCode,
}

#[async_trait::async_trait]
impl VisitorCredentialSource for Gate {
    async fn credential_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, StoreError> {
        self.credentials_started.fetch_add(1, Ordering::SeqCst);
        self.credentials_release.acquire().await.unwrap().forget();
        Ok(Some(SecretValue::new("catalog-test-credential".into())))
    }
}

async fn read_gate(state: &Gate, headers: &HeaderMap) {
    assert_eq!(
        headers.get(header::AUTHORIZATION).unwrap(),
        "Bearer catalog-test-credential"
    );
    state.reads_started.fetch_add(1, Ordering::SeqCst);
    state.reads_release.acquire().await.unwrap().forget();
}

async fn catalog(State(state): State<Arc<Gate>>, headers: HeaderMap) -> &'static str {
    read_gate(&state, &headers).await;
    state.catalog_body
}

async fn services(
    State(state): State<Arc<Gate>>,
    headers: HeaderMap,
) -> (StatusCode, &'static str) {
    read_gate(&state, &headers).await;
    (
        state.services_status,
        r#"{"services":[{"id":"existing","slug":"service","is_active":true}]}"#,
    )
}

struct Fixture {
    gate: Arc<Gate>,
    broker: NyxidServiceConnectionBroker,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(catalog_body: &'static str, services_status: StatusCode) -> Self {
        let gate = Arc::new(Gate {
            credentials_started: AtomicUsize::new(0),
            credentials_release: Semaphore::new(0),
            reads_started: AtomicUsize::new(0),
            reads_release: Semaphore::new(0),
            catalog_body,
            services_status,
        });
        let app = Router::new()
            .route("/api/v1/services", get(catalog))
            .route("/api/v1/user-services", get(services))
            .with_state(gate.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let broker = NyxidServiceConnectionBroker::new(
            reqwest::Client::new(),
            format!("http://{}", listener.local_addr().unwrap()),
            "https://nyx.example",
            gate.clone(),
        )
        .unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            gate,
            broker,
            server,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn metadata_reads_overlap_after_owner_credential_resolution() {
    let fixture = Fixture::new(
        r#"{"services":[{"slug":"service","name":"Selected service","service_type":"http","is_active":true}]}"#,
        StatusCode::OK,
    )
    .await;
    let owner = SubjectId::new("owner");
    let requested = [ServiceId::new("service")];
    let read = fixture.broker.describe_services(&owner, &requested);
    tokio::pin!(read);
    tokio::select! {
        result = &mut read => panic!("credential-gated read completed: {result:?}"),
        reached = tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.gate.credentials_started.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        }) => reached.unwrap(),
    }
    assert_eq!(fixture.gate.reads_started.load(Ordering::SeqCst), 0);
    fixture.gate.credentials_release.add_permits(1);
    tokio::select! {
        result = &mut read => panic!("response-gated read completed: {result:?}"),
        reached = tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.gate.reads_started.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
        }) => reached.expect("both owner-bound reads must start before either response returns"),
    }
    fixture.gate.reads_release.add_permits(2);
    let rows = read.await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].service, requested[0]);
    assert_eq!(rows[0].display_name.as_deref(), Some("Selected service"));
    assert_eq!(rows[0].connection_mode, ServiceConnectionMode::Manual);
    assert_eq!(
        rows[0].setup_url.as_deref(),
        Some("https://nyx.example/keys/existing")
    );
}

#[tokio::test]
async fn overlapping_metadata_reads_preserve_failure_precedence() {
    for (catalog, decode_failure) in [("invalid-json", true), (r#"{"services":[]}"#, false)] {
        let fixture = Fixture::new(catalog, StatusCode::SERVICE_UNAVAILABLE).await;
        fixture.gate.credentials_release.add_permits(1);
        fixture.gate.reads_release.add_permits(2);
        let error = fixture
            .broker
            .describe_services(&SubjectId::new("owner"), &[ServiceId::new("service")])
            .await
            .unwrap_err();
        if decode_failure {
            assert!(matches!(error, ServiceConnectionError::Malformed(_)));
        } else {
            assert!(matches!(error, ServiceConnectionError::Unavailable(_)));
        }
        assert_eq!(fixture.gate.reads_started.load(Ordering::SeqCst), 2);
    }
}
