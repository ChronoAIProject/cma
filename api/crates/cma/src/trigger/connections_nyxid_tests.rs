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
    BrokerHandoffState, ServiceConnectionBroker, ServiceConnectionError, ServiceRequirementState,
};
use layer2_core::{ServiceId, StoreError, SubjectId};
use serde_json::{json, Value};

use super::connections_nyxid::{NyxidServiceConnectionBroker, VisitorCredentialSource};
use crate::auth::persistence::SecretValue;

const VISITOR_TOKEN: &str = "visitor-access-secret";
const EXPIRES_AT: &str = "2026-09-05T12:15:00Z";

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
    create_reply: Mutex<Reply>,
    link_reply: Mutex<Reply>,
    services_reply: Mutex<Reply>,
    creates: Mutex<Vec<CreateRequest>>,
    link_reads: Mutex<Vec<(String, Option<String>)>>,
    service_reads: Mutex<Vec<Option<String>>>,
}

impl ProviderState {
    fn new() -> Self {
        Self {
            create_reply: Mutex::new(Reply::json(json!({
                "id": "link-one",
                "connect_url": "https://nyx.example/connect/nyx_clk_secret",
                "expires_at": EXPIRES_AT,
            }))),
            link_reply: Mutex::new(link_reply("pending")),
            services_reply: Mutex::new(Reply::json(json!({ "services": [] }))),
            creates: Mutex::new(Vec::new()),
            link_reads: Mutex::new(Vec::new()),
            service_reads: Mutex::new(Vec::new()),
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
    state
        .creates
        .lock()
        .expect("create requests lock")
        .push(CreateRequest {
            authorization: authorization(&headers),
            body: serde_json::from_slice(&body).unwrap_or(Value::Null),
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
        tokens,
    )
}

fn link_reply(status: &str) -> Reply {
    Reply::json(json!({
        "id": "link-one",
        "status": status,
        "service_name": "GitHub",
        "service_slug": "github",
        "expires_at": EXPIRES_AT,
    }))
}

#[tokio::test]
async fn requirements_preserve_order_and_map_active_inactive_missing_and_unknown() {
    let provider = Provider::start().await;
    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            { "id": "by-id", "slug": "alias", "is_active": true },
            { "id": "inactive-id", "slug": "inactive", "is_active": false },
            { "id": "unknown-id", "slug": "unknown" }
        ]
    })));
    let tokens = Tokens::available();
    let owner = SubjectId::new("visitor-one");
    let wanted = ["by-id", "inactive", "missing", "unknown"].map(ServiceId::new);

    let requirements = broker(&provider, Arc::clone(&tokens))
        .requirements(&owner, &wanted)
        .await
        .expect("requirements resolve");

    assert_eq!(
        requirements
            .iter()
            .map(|requirement| (&requirement.service, requirement.state))
            .collect::<Vec<_>>(),
        vec![
            (&wanted[0], ServiceRequirementState::Satisfied),
            (&wanted[1], ServiceRequirementState::Inactive),
            (&wanted[2], ServiceRequirementState::Missing),
            (&wanted[3], ServiceRequirementState::Unknown),
        ]
    );
    assert_eq!(
        *provider
            .state
            .service_reads
            .lock()
            .expect("service requests lock"),
        vec![Some(format!("Bearer {VISITOR_TOKEN}"))]
    );
    assert_eq!(
        *tokens.owners.lock().expect("token owner lock"),
        vec![owner]
    );
}

#[tokio::test]
async fn handoff_creation_is_visitor_scoped_callback_free_and_exactly_fifteen_minutes() {
    let provider = Provider::start().await;
    let tokens = Tokens::available();

    let handoff = broker(&provider, tokens)
        .create_handoff(
            &SubjectId::new("visitor-one"),
            &ServiceId::new("github"),
            "intent-one",
        )
        .await
        .expect("handoff is created");

    assert_eq!(handoff.external_id, "link-one");
    assert_eq!(
        handoff.hosted_url,
        "https://nyx.example/connect/nyx_clk_secret"
    );
    assert_eq!(handoff.expires_at, EXPIRES_AT);
    let creates = provider.state.creates.lock().expect("create requests lock");
    assert_eq!(creates.len(), 1);
    assert_eq!(
        creates[0].authorization.as_deref(),
        Some("Bearer visitor-access-secret")
    );
    assert_eq!(
        creates[0].body,
        json!({ "service_slug": "github", "expires_in": 900 })
    );
    assert!(creates[0].body.get("callback_url").is_none());
}

#[tokio::test]
async fn provider_terminal_states_map_without_polling_user_services() {
    let provider = Provider::start().await;
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    for (wire, expected) in [
        ("pending", BrokerHandoffState::Pending),
        ("expired", BrokerHandoffState::Expired),
        ("cancelled", BrokerHandoffState::Cancelled),
    ] {
        provider.state.set_link_reply(link_reply(wire));
        assert_eq!(
            broker
                .handoff_state(&owner, "link-one", &service)
                .await
                .expect("provider state maps"),
            expected
        );
    }
    assert!(provider
        .state
        .service_reads
        .lock()
        .expect("service requests lock")
        .is_empty());
}

#[tokio::test]
async fn completed_handoff_is_satisfied_only_for_the_exact_active_user_service() {
    let provider = Provider::start().await;
    provider.state.set_link_reply(Reply::json(json!({
        "id": "link-one",
        "status": "completed",
        "service_name": "GitHub",
        "service_slug": "github",
        "expires_at": EXPIRES_AT,
        "connected_service": { "id": "user-service-one", "slug": "visitor-github" }
    })));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            { "id": "user-service-one", "slug": "visitor-github", "is_active": true }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("active completion resolves"),
        BrokerHandoffState::Satisfied
    );

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            { "id": "user-service-one", "slug": "visitor-github", "is_active": false }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("inactive completion resolves"),
        BrokerHandoffState::Failed
    );
}

#[tokio::test]
async fn provider_and_credential_failures_are_typed_and_sanitized() {
    let provider = Provider::start().await;
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_link_reply(Reply::error(
        StatusCode::NOT_FOUND,
        r#"{"message":"private provider detail"}"#,
    ));
    assert_eq!(
        broker(&provider, Tokens::available())
            .handoff_state(&owner, "link-one", &service)
            .await,
        Err(ServiceConnectionError::NotFound)
    );

    provider.state.set_create_reply(Reply::error(
        StatusCode::FORBIDDEN,
        r#"{"message":"private policy detail"}"#,
    ));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(ServiceConnectionError::Refused)
    );

    provider.state.set_services_reply(Reply::error(
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"message":"database host and private detail"}"#,
    ));
    let unavailable = broker(&provider, Tokens::available())
        .requirements(&owner, std::slice::from_ref(&service))
        .await
        .expect_err("provider outage is unavailable");
    assert_eq!(
        unavailable,
        ServiceConnectionError::Unavailable("NyxID request failed".into())
    );
    assert!(!unavailable.to_string().contains("database host"));

    let credential_error = broker(
        &provider,
        Tokens::failing("encrypted store and private detail"),
    )
    .requirements(&owner, std::slice::from_ref(&service))
    .await
    .expect_err("credential outage is unavailable");
    assert_eq!(
        credential_error,
        ServiceConnectionError::Unavailable("visitor credential lookup failed".into())
    );
    assert!(!credential_error.to_string().contains("encrypted store"));

    assert_eq!(
        broker(&provider, Tokens::missing())
            .requirements(&owner, &[service])
            .await,
        Err(ServiceConnectionError::Refused)
    );
}

#[tokio::test]
async fn malformed_provider_payloads_fail_closed_without_echoing_the_body() {
    let provider = Provider::start().await;
    provider.state.set_create_reply(Reply::error(
        StatusCode::OK,
        r#"{"private_secret":"must-not-escape"}"#,
    ));

    let error = broker(&provider, Tokens::available())
        .create_handoff(
            &SubjectId::new("visitor-one"),
            &ServiceId::new("github"),
            "intent-one",
        )
        .await
        .expect_err("malformed response is refused");

    assert_eq!(
        error,
        ServiceConnectionError::Malformed("NyxID response was not usable".into())
    );
    assert!(!error.to_string().contains("must-not-escape"));
}
