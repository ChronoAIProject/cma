use std::sync::{Arc, Mutex};

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use layer2_core::trigger_domain::{
    BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError, ServiceConnectionMode,
    ServiceRequirementState,
};
use layer2_core::{ServiceId, StoreError, SubjectId};
use serde_json::{json, Value};

use super::connections_nyxid::{NyxidServiceConnectionBroker, VisitorCredentialSource};
use crate::auth::persistence::SecretValue;

const VISITOR_TOKEN: &str = "visitor-access-secret";
const PROVIDER_EXPIRES_AT: &str = "2026-09-05T12:15:00.731Z";
const CANONICAL_EXPIRES_AT: &str = "2026-09-05T12:15:00Z";
const CONNECT_TOKEN: &str =
    "nyx_clk_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: String,
}

impl Reply {
    fn json(value: Value) -> Self {
        Self {
            status: StatusCode::OK,
            body: value.to_string(),
        }
    }

    fn error(status: StatusCode, body: &str) -> Self {
        Self {
            status,
            body: body.to_owned(),
        }
    }

    fn response(&self) -> Response {
        (
            self.status,
            [(header::CONTENT_TYPE, "application/json")],
            self.body.clone(),
        )
            .into_response()
    }
}

#[derive(Debug)]
struct CreateRequest {
    authorization: Option<String>,
    body: Value,
}

struct ProviderState {
    registered_callback: Mutex<Option<String>>,
    create_reply: Mutex<Reply>,
    link_reply: Mutex<Reply>,
    services_reply: Mutex<Reply>,
    catalog_reply: Mutex<Reply>,
    credential_replies: Mutex<std::collections::HashMap<String, Reply>>,
    creates: Mutex<Vec<CreateRequest>>,
    link_reads: Mutex<Vec<(String, Option<String>)>>,
    service_reads: Mutex<Vec<Option<String>>>,
    catalog_reads: Mutex<Vec<Option<String>>>,
    credential_reads: Mutex<Vec<Option<String>>>,
    readiness_reply: Mutex<Reply>,
}

impl ProviderState {
    fn new() -> Self {
        Self {
            registered_callback: Mutex::new(None),
            create_reply: Mutex::new(Reply::json(json!({
                "id": "link-one",
                "connect_url": trusted_connect_url(),
                "expires_at": PROVIDER_EXPIRES_AT,
            }))),
            link_reply: Mutex::new(link_reply("pending")),
            services_reply: Mutex::new(Reply::json(json!({ "services": [] }))),
            catalog_reply: Mutex::new(Reply::json(json!({ "services": [] }))),
            credential_replies: Mutex::new(std::collections::HashMap::new()),
            creates: Mutex::new(Vec::new()),
            link_reads: Mutex::new(Vec::new()),
            service_reads: Mutex::new(Vec::new()),
            catalog_reads: Mutex::new(Vec::new()),
            credential_reads: Mutex::new(Vec::new()),
            readiness_reply: Mutex::new(Reply::json(
                json!({"revision":"nyxid-assistant-readiness.v1","capabilities":[{"capabilityId":"api-github","connectionState":"connected","grantState":"missing"}]}),
            )),
        }
    }

    fn set_create_reply(&self, reply: Reply) {
        *self.create_reply.lock().expect("create reply lock") = reply;
    }

    fn set_link_reply(&self, reply: Reply) {
        *self.link_reply.lock().expect("link reply lock") = reply;
    }

    fn set_services_reply(&self, reply: Reply) {
        *self.services_reply.lock().expect("services reply lock") = reply;
    }
}

struct Provider {
    base_url: String,
    state: Arc<ProviderState>,
    server: tokio::task::JoinHandle<()>,
}

impl Provider {
    async fn start() -> Self {
        let state = Arc::new(ProviderState::new());
        let app = Router::new()
            .route("/api/v1/connect-links", post(create_link))
            .route("/api/v1/connect-links/{id}", get(read_link))
            .route("/api/v1/user-services", get(read_services))
            .route("/api/v1/services", get(read_catalog))
            .route("/api/v1/keys/{id}", get(read_credential))
            .route("/api/v1/assistant/readiness", get(read_readiness))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("provider fixture binds");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("provider fixture address")
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("provider fixture serves");
        });
        Self {
            base_url,
            state,
            server,
        }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn create_link(
    State(state): State<Arc<ProviderState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if state
        .registered_callback
        .lock()
        .unwrap()
        .as_deref()
        .is_some_and(|registered| {
            body.get("callback_url").and_then(Value::as_str) != Some(registered)
        })
    {
        return Reply::error(StatusCode::BAD_REQUEST, "invalid_redirect_uri").response();
    }
    state
        .creates
        .lock()
        .expect("create requests lock")
        .push(CreateRequest {
            authorization: authorization(&headers),
            body,
        });
    state
        .create_reply
        .lock()
        .expect("create reply lock")
        .response()
}

async fn read_link(
    State(state): State<Arc<ProviderState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    state
        .link_reads
        .lock()
        .expect("link requests lock")
        .push((id, authorization(&headers)));
    state.link_reply.lock().expect("link reply lock").response()
}

async fn read_services(State(state): State<Arc<ProviderState>>, headers: HeaderMap) -> Response {
    state
        .service_reads
        .lock()
        .expect("service requests lock")
        .push(authorization(&headers));
    state
        .services_reply
        .lock()
        .expect("services reply lock")
        .response()
}

async fn read_catalog(State(state): State<Arc<ProviderState>>, headers: HeaderMap) -> Response {
    state
        .catalog_reads
        .lock()
        .unwrap()
        .push(authorization(&headers));
    state.catalog_reply.lock().unwrap().response()
}

async fn read_credential(
    State(state): State<Arc<ProviderState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    assert_eq!(
        authorization(&headers),
        Some(format!("Bearer {VISITOR_TOKEN}"))
    );
    state
        .credential_reads
        .lock()
        .unwrap()
        .push(authorization(&headers));
    if let Some(reply) = state.credential_replies.lock().unwrap().get(&id) {
        return reply.response();
    }
    let listing: Value = serde_json::from_str(&state.services_reply.lock().unwrap().body).unwrap();
    let rows = listing
        .as_array()
        .or_else(|| listing.get("services").and_then(Value::as_array));
    let row = rows.and_then(|rows| rows.iter().find(|row| row["id"] == id));
    match row {
        Some(row) => Reply::json(json!({"id":id,"slug":row["slug"],"is_active":true,"status":"active","connected":true,"credential_missing":false})).response(),
        None => Reply::error(StatusCode::NOT_FOUND, "missing").response(),
    }
}

async fn read_readiness(State(state): State<Arc<ProviderState>>, headers: HeaderMap) -> Response {
    assert_eq!(
        authorization(&headers),
        Some(format!("Bearer {VISITOR_TOKEN}"))
    );
    state.readiness_reply.lock().unwrap().response()
}

fn authorization(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

struct Tokens {
    token: Option<String>,
    failure: Option<StoreError>,
    owners: Mutex<Vec<SubjectId>>,
}

impl Tokens {
    fn available() -> Arc<Self> {
        Arc::new(Self {
            token: Some(VISITOR_TOKEN.to_owned()),
            failure: None,
            owners: Mutex::new(Vec::new()),
        })
    }

    fn missing() -> Arc<Self> {
        Arc::new(Self {
            token: None,
            failure: None,
            owners: Mutex::new(Vec::new()),
        })
    }

    fn failing(detail: &str) -> Arc<Self> {
        Arc::new(Self {
            token: None,
            failure: Some(StoreError::Unavailable(detail.to_owned())),
            owners: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait::async_trait]
impl VisitorCredentialSource for Tokens {
    async fn credential_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, StoreError> {
        self.owners
            .lock()
            .expect("token owner lock")
            .push(owner.clone());
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        Ok(self.token.clone().map(SecretValue::new))
    }
}

fn broker(provider: &Provider, tokens: Arc<Tokens>) -> NyxidServiceConnectionBroker {
    NyxidServiceConnectionBroker::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client builds"),
        format!("{}/", provider.base_url),
        "https://nyx.example",
        tokens,
    )
    .expect("broker configuration is valid")
}

fn broker_with_callback(provider: &Provider, tokens: Arc<Tokens>) -> NyxidServiceConnectionBroker {
    NyxidServiceConnectionBroker::new_with_callback(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client builds"),
        format!("{}/", provider.base_url),
        "https://nyx.example",
        Some("https://cma.example"),
        tokens,
    )
    .expect("broker configuration is valid")
}

fn link_reply(status: &str) -> Reply {
    Reply::json(json!({
        "id": "link-one",
        "status": status,
        "service_name": "GitHub",
        "service_slug": "github",
        "expires_at": PROVIDER_EXPIRES_AT,
    }))
}

fn trusted_connect_url() -> String {
    format!("https://nyx.example/connect/{CONNECT_TOKEN}")
}

fn create_reply(connect_url: impl Into<String>) -> Reply {
    Reply::json(json!({
        "id": "link-one",
        "connect_url": connect_url.into(),
        "expires_at": PROVIDER_EXPIRES_AT,
    }))
}

fn malformed_error() -> ServiceConnectionError {
    ServiceConnectionError::Malformed("NyxID response was not usable".into())
}

#[path = "connections_nyxid_tests/catalog.rs"]
mod catalog;
#[path = "connections_nyxid_tests/failures.rs"]
mod failures;
#[path = "connections_nyxid_tests/handoffs.rs"]
mod handoffs;
#[path = "connections_nyxid_tests/requirements.rs"]
mod requirements;
