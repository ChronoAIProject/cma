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

#[path = "connections_nyxid_tests/deadline.rs"]
mod deadline;
#[path = "connections_nyxid_tests/github.rs"]
mod github;
mod github_repository;
pub(crate) use github_repository::GithubRepositoryFixture;

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
    github_proxy_reads: Mutex<Vec<(String, String)>>,
    github_proxy_scopes: Mutex<Option<String>>,
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
            github_proxy_reads: Mutex::new(Vec::new()),
            github_proxy_scopes: Mutex::new(None),
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
            .route("/api/v1/proxy/{id}/user", get(read_github_proxy))
            .route(
                "/api/v1/proxy/{id}/repos/{owner}/{name}",
                get(read_github_repository),
            )
            .route(
                "/api/v1/proxy/{id}/repos/{owner}/{name}/commits/{reference}",
                get(read_github_reference),
            )
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

async fn read_github_proxy(
    State(state): State<Arc<ProviderState>>,
    Path(catalog): Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    assert_eq!(
        authorization(&headers),
        Some(format!("Bearer {VISITOR_TOKEN}"))
    );
    let selected = query.get("_nyxid_via").unwrap().clone();
    let login = format!("fixture-{selected}");
    state
        .github_proxy_reads
        .lock()
        .unwrap()
        .push((catalog, selected));
    let mut response = Reply::json(json!({"id": 19, "login": login})).response();
    if let Some(scopes) = state.github_proxy_scopes.lock().unwrap().as_deref() {
        response
            .headers_mut()
            .insert("x-oauth-scopes", scopes.parse().unwrap());
    }
    response
}

async fn read_github_repository(
    State(state): State<Arc<ProviderState>>,
    Path((catalog, owner, name)): Path<(String, String, String)>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let identity = read_github_proxy(State(state), Path(catalog), query, headers).await;
    let mut response = if owner == "example"
        && matches!(name.as_str(), "project" | "restricted" | "private-project")
    {
        Reply::json(json!({
            "full_name": format!("{owner}/{name}"),
            "default_branch": "main",
            "private": name == "private-project",
            "disabled": false,
            "archived": false,
            "permissions": {"push": name == "project"},
        }))
        .response()
    } else {
        Reply::error(StatusCode::NOT_FOUND, "repository missing").response()
    };
    if let Some(scopes) = identity.headers().get("x-oauth-scopes") {
        response
            .headers_mut()
            .insert("x-oauth-scopes", scopes.clone());
    }
    response
}

async fn read_github_reference(
    State(state): State<Arc<ProviderState>>,
    Path((catalog, owner, name, reference)): Path<(String, String, String, String)>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    assert_eq!(owner, "example");
    assert!(matches!(name.as_str(), "project" | "private-project"));
    let identity = read_github_proxy(State(state), Path(catalog), query, headers).await;
    let commit = "a".repeat(40);
    let mut response =
        if matches!(reference.as_str(), "main" | "branch/synthetic") || reference == commit {
            Reply::json(json!({"sha":commit})).response()
        } else {
            Reply::error(StatusCode::NOT_FOUND, "ref missing").response()
        };
    if let Some(scopes) = identity.headers().get("x-oauth-scopes") {
        response
            .headers_mut()
            .insert("x-oauth-scopes", scopes.clone());
    }
    response
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

#[tokio::test]
async fn catalog_metadata_uses_exact_owner_visible_types_and_configured_setup_origin() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() = Reply::json(json!({ "services": [
        {"id":"provider-internal-id","slug":"github","name":"GitHub","service_type":"http","is_active":true,"base_url":"http://internal.example","oauth_client_secret":"not-projected"},
        {"slug":"workstation","name":"Development host","service_type":"ssh","is_active":true,"homepage_url":"https://untrusted.example/setup"},
        {"slug":"future","name":"Future service","service_type":"future_protocol","is_active":true},
        {"slug":"inactive","name":"Inactive service","service_type":"http","is_active":false},
        {"slug":"unreported","name":"Unreported service"}
    ] }));
    let tokens = Tokens::available();
    let broker = broker(&provider, tokens.clone());
    let owner = SubjectId::new("visitor-metadata");
    let services = [
        "github",
        "workstation",
        "future",
        "inactive",
        "unreported",
        "provider-internal-id",
        "missing",
    ]
    .map(ServiceId::new);
    let metadata = broker.describe_services(&owner, &services).await.unwrap();
    assert_eq!(metadata.len(), services.len());
    assert_eq!(metadata[0].display_name.as_deref(), Some("GitHub"));
    assert_eq!(metadata[0].connection_mode, ServiceConnectionMode::Hosted);
    assert_eq!(metadata[0].setup_url, None);
    assert_eq!(metadata[1].connection_mode, ServiceConnectionMode::Manual);
    assert_eq!(
        metadata[1].setup_url.as_deref(),
        Some("https://nyx.example/keys")
    );
    for item in &metadata[2..] {
        assert_eq!(item.connection_mode, ServiceConnectionMode::Unknown);
        assert_eq!(item.setup_url, None);
    }
    assert_eq!(metadata[5].display_name, None);
    assert_eq!(metadata[6].display_name, None);
    assert_eq!(*tokens.owners.lock().unwrap(), vec![owner]);
    assert_eq!(
        *provider.state.catalog_reads.lock().unwrap(),
        vec![Some(format!("Bearer {VISITOR_TOKEN}"))]
    );
    assert_eq!(provider.state.service_reads.lock().unwrap().len(), 1);
    assert!(provider.state.creates.lock().unwrap().is_empty());
    let printed = format!("{metadata:?}");
    for secret in [
        VISITOR_TOKEN,
        "not-projected",
        "internal.example",
        "untrusted.example",
    ] {
        assert!(!printed.contains(secret));
    }
}

#[tokio::test]
async fn existing_connections_link_to_the_exact_nyxid_repair_page() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() = Reply::json(json!({"services":[
        {"slug":"api-github","name":"GitHub","service_type":"http","is_active":true}
    ]}));
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[
            {"id":"github-existing","slug":"api-github","is_active":true}
        ]})));
    let metadata = broker(&provider, Tokens::available())
        .describe_services(
            &SubjectId::new("visitor-one"),
            &[
                ServiceId::new("api-github"),
                ServiceId::new("github-existing"),
            ],
        )
        .await
        .unwrap();
    for service in metadata {
        assert_eq!(service.display_name.as_deref(), Some("GitHub"));
        assert_eq!(service.connection_mode, ServiceConnectionMode::Manual);
        assert_eq!(
            service.setup_url.as_deref(),
            Some("https://nyx.example/keys/github-existing")
        );
    }
    assert!(provider.state.creates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn active_github_credential_rows_without_material_stay_blocked() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[
            {"id":"github-existing","slug":"api-github","is_active":true}
        ]})));
    let broker = broker(&provider, Tokens::available());
    for (connection, expected) in [
        ("verifying", ServiceRequirementState::Missing),
        ("connecting", ServiceRequirementState::Missing),
        ("unknown", ServiceRequirementState::Unknown),
        ("connected", ServiceRequirementState::Satisfied),
    ] {
        *provider.state.readiness_reply.lock().unwrap() = Reply::json(json!({
            "revision":"nyxid-assistant-readiness.v1",
            "capabilities":[{"capabilityId":"api-github","connectionState":connection,"grantState":"missing"}]
        }));
        let requirements = broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")],
            )
            .await
            .unwrap();
        assert_eq!(requirements[0].state, expected);
    }
    *provider.state.readiness_reply.lock().unwrap() =
        Reply::json(json!({"revision":"future","capabilities":[]}));
    assert_eq!(
        broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")]
            )
            .await
            .unwrap_err(),
        malformed_error()
    );
}

#[tokio::test]
async fn catalog_metadata_rejects_duplicate_malformed_and_oversized_catalogs() {
    let provider = Provider::start().await;
    let broker = broker(&provider, Tokens::available());
    let row = json!({"slug":"github","name":"GitHub","service_type":"http","is_active":true});
    for catalog in [
        json!({"services":[row.clone(),row.clone()]}),
        json!({"services":[{"slug":"github","name":""}]}),
        json!({"services":[{"slug":"github","name":"x".repeat(257)}]}),
        json!({"services":[{"slug":"github\nsecret","name":"GitHub"}]}),
        json!({"services": vec![row; 1001]}),
        json!({"services":[], "ignored":"x".repeat(1024 * 1024)}),
        json!({"items":[]}),
    ] {
        *provider.state.catalog_reply.lock().unwrap() = Reply::json(catalog);
        assert_eq!(
            broker
                .describe_services(&SubjectId::new("visitor"), &[ServiceId::new("github")])
                .await,
            Err(malformed_error())
        );
    }
}

#[tokio::test]
async fn catalog_metadata_does_not_read_without_requirements_or_owner_credentials() {
    let provider = Provider::start().await;
    let tokens = Tokens::missing();
    let broker = broker(&provider, tokens.clone());
    let owner = SubjectId::new("visitor");
    assert!(broker
        .describe_services(&owner, &[])
        .await
        .unwrap()
        .is_empty());
    assert!(tokens.owners.lock().unwrap().is_empty());
    assert_eq!(
        broker
            .describe_services(&owner, &[ServiceId::new("github")])
            .await,
        Err(ServiceConnectionError::Refused)
    );
    assert!(provider.state.catalog_reads.lock().unwrap().is_empty());
}

#[test]
fn broker_requires_one_canonical_https_browser_origin() {
    for origin in [
        "http://nyx.example",
        "https://nyx.example/",
        "https://user@nyx.example",
        "https://nyx.example/connect",
        "https://nyx.example?next=elsewhere",
    ] {
        assert!(
            NyxidServiceConnectionBroker::new(
                reqwest::Client::new(),
                "https://identity.example",
                origin,
                Tokens::available(),
            )
            .is_err(),
            "accepted unsafe browser origin {origin}"
        );
    }
}

#[tokio::test]
async fn requirements_preserve_order_and_honor_provider_credential_access() {
    let provider = Provider::start().await;
    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "by-id",
                "slug": "alias",
                "is_active": true,
                "credential_source": { "type": "personal" }
            },
            {
                "id": "org-id",
                "slug": "org-service",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-one",
                    "org_name": "Allowed org",
                    "role": "member",
                    "allowed": true
                }
            },
            {
                "id": "denied-id",
                "slug": "denied",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-two",
                    "org_name": "Read-only org",
                    "role": "viewer",
                    "allowed": false
                }
            }
        ]
    })));
    let tokens = Tokens::available();
    let owner = SubjectId::new("visitor-one");
    let wanted = ["by-id", "org-service", "denied", "missing"].map(ServiceId::new);

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
            (&wanted[1], ServiceRequirementState::Satisfied),
            (&wanted[2], ServiceRequirementState::Unknown),
            (&wanted[3], ServiceRequirementState::Missing),
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
async fn legacy_bare_listing_maps_inactive_and_missing_activity_to_closed_states() {
    let provider = Provider::start().await;
    provider.state.set_services_reply(Reply::json(json!([
        { "id": "inactive-id", "slug": "inactive", "is_active": false },
        { "id": "unknown-id", "slug": "unknown" }
    ])));
    let wanted = ["inactive", "unknown", "missing"].map(ServiceId::new);

    let requirements = broker(&provider, Tokens::available())
        .requirements(&SubjectId::new("visitor-one"), &wanted)
        .await
        .expect("legacy requirements resolve");

    assert_eq!(
        requirements
            .iter()
            .map(|requirement| requirement.state)
            .collect::<Vec<_>>(),
        vec![
            ServiceRequirementState::Inactive,
            ServiceRequirementState::Unknown,
            ServiceRequirementState::Missing,
        ]
    );
}

#[tokio::test]
async fn active_routing_rows_require_present_credentials_by_slug_and_id() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-routing-row","slug":"api-github","is_active":true},
        {"id":"ornn-row","slug":"ornn-api","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let services = ["api-github", "github-routing-row", "ornn-api"].map(ServiceId::new);
    for (reply, expected) in [
        (
            Reply::json(
                json!({"id":"github-routing-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":true}),
            ),
            ServiceRequirementState::Missing,
        ),
        (
            Reply::error(StatusCode::NOT_FOUND, "private provider detail"),
            ServiceRequirementState::Missing,
        ),
        (
            Reply::json(
                json!({"id":"github-routing-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":false}),
            ),
            ServiceRequirementState::Satisfied,
        ),
    ] {
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("github-routing-row".into(), reply);
        let requirements = broker.requirements(&owner, &services).await.unwrap();
        assert_eq!(
            requirements.iter().map(|r| r.state).collect::<Vec<_>>(),
            vec![expected, expected, ServiceRequirementState::Satisfied]
        );
    }
    assert_eq!(
        *provider.state.credential_reads.lock().unwrap(),
        vec![Some(format!("Bearer {VISITOR_TOKEN}")); 9]
    );
}

#[tokio::test]
async fn credential_metadata_failures_never_look_like_satisfied_connections() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    for reply in [
        Reply::error(StatusCode::TOO_MANY_REQUESTS, "private provider response"),
        Reply::error(StatusCode::SERVICE_UNAVAILABLE, "private provider response"),
        Reply::json(
            json!({"id":"another-user-service","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":false}),
        ),
        Reply::json(
            json!({"id":"github-row","slug":"api-github","is_active":true,"status":"active","connected":true}),
        ),
    ] {
        let before = provider.state.credential_reads.lock().unwrap().len();
        let expected_reads = if reply.status == StatusCode::TOO_MANY_REQUESTS {
            2
        } else {
            1
        };
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("github-row".into(), reply);
        let error = broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ServiceConnectionError::Unavailable(_) | ServiceConnectionError::Malformed(_)
        ));
        assert!(!error.to_string().contains("private provider"));
        assert_eq!(
            provider.state.credential_reads.lock().unwrap().len(),
            before + expected_reads,
            "only an explicit provider throttle may repeat the credential read"
        );
    }
    let before_denied = provider.state.credential_reads.lock().unwrap().len();
    provider.state.set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true,"credential_source":{"type":"org","allowed":false}}
    ] })));
    let requirements = broker
        .requirements(
            &SubjectId::new("visitor-one"),
            &[ServiceId::new("api-github")],
        )
        .await
        .unwrap();
    assert_eq!(requirements[0].state, ServiceRequirementState::Unknown);
    assert_eq!(
        provider.state.credential_reads.lock().unwrap().len(),
        before_denied,
        "denied organization credentials must not be probed"
    );
}

#[tokio::test]
async fn completed_handoff_still_requires_present_credentials() {
    let provider = Provider::start().await;
    provider.state.set_link_reply(Reply::json(json!({
        "id":"link-one","status":"completed","service_slug":"api-github",
        "expires_at":PROVIDER_EXPIRES_AT,
        "connected_service":{"id":"github-row","slug":"api-github"}
    })));
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    provider.state.credential_replies.lock().unwrap().insert("github-row".into(),
        Reply::json(json!({"id":"github-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":true})));
    assert_eq!(
        broker
            .handoff_state(
                &SubjectId::new("visitor-one"),
                "link-one",
                &ServiceId::new("api-github")
            )
            .await
            .unwrap(),
        BrokerHandoffState::Failed
    );
    provider
        .state
        .credential_replies
        .lock()
        .unwrap()
        .remove("github-row");
    assert_eq!(
        broker
            .handoff_state(
                &SubjectId::new("visitor-one"),
                "link-one",
                &ServiceId::new("api-github")
            )
            .await
            .unwrap(),
        BrokerHandoffState::Satisfied
    );
}

#[tokio::test]
async fn every_required_service_checks_credential_state_before_becoming_satisfied() {
    let provider = Provider::start().await;
    let cases = [
        (
            "pending",
            json!({"status":"pending_auth"}),
            ServiceRequirementState::Missing,
        ),
        (
            "expired",
            json!({"connection_status":"expired"}),
            ServiceRequirementState::Missing,
        ),
        (
            "empty",
            json!({"credential_missing":true}),
            ServiceRequirementState::Missing,
        ),
        (
            "disconnected",
            json!({"connected":false}),
            ServiceRequirementState::Missing,
        ),
        (
            "disabled",
            json!({"is_active":false}),
            ServiceRequirementState::Inactive,
        ),
        (
            "unknown",
            json!({"status":"future"}),
            ServiceRequirementState::Unknown,
        ),
        (
            "internal",
            json!({"auth_method":"none","credential_missing":false}),
            ServiceRequirementState::Satisfied,
        ),
        (
            "node",
            json!({"node_id":"node-one","node_status":"offline"}),
            ServiceRequirementState::Inactive,
        ),
        ("active", json!({}), ServiceRequirementState::Satisfied),
    ];
    let rows = cases
        .iter()
        .map(|(id, _, _)| json!({"id":id,"slug":id,"is_active":true}))
        .collect::<Vec<_>>();
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":rows})));
    for (id, fields, _) in &cases {
        let mut key = json!({"id":id,"slug":id,"is_active":true,"status":"active","connected":true,"credential_missing":false});
        key.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert(id.to_string(), Reply::json(key));
    }
    let services = cases
        .iter()
        .map(|(id, _, _)| ServiceId::new(*id))
        .collect::<Vec<_>>();
    let requirements = broker(&provider, Tokens::available())
        .requirements(&SubjectId::new("visitor-one"), &services)
        .await
        .unwrap();
    assert_eq!(
        requirements.iter().map(|r| r.state).collect::<Vec<_>>(),
        cases.iter().map(|(_, _, state)| *state).collect::<Vec<_>>()
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
    assert_eq!(handoff.hosted_url, trusted_connect_url());
    assert_eq!(handoff.expires_at, CANONICAL_EXPIRES_AT);
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
async fn handoff_creation_uses_the_registered_static_callback_for_distinct_intents() {
    let provider = Provider::start().await;
    *provider.state.registered_callback.lock().unwrap() =
        Some("https://cma.example/connection-return".into());
    let broker = broker_with_callback(&provider, Tokens::available());
    for intent in ["intent-one", "intent-two"] {
        broker
            .create_handoff(
                &SubjectId::new("visitor-one"),
                &ServiceId::new("github"),
                intent,
            )
            .await
            .expect("the provider accepts the exact registered callback for every intent");
    }
    let creates = provider.state.creates.lock().expect("create requests lock");
    assert_eq!(creates.len(), 2);
    for request in creates.iter() {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer visitor-access-secret")
        );
        assert_eq!(
            request.body,
            json!({
                "service_slug": "github",
                "expires_in": 900,
                "callback_url": "https://cma.example/connection-return"
            })
        );
    }
}

#[tokio::test]
async fn handoff_creation_rejects_foreign_insecure_or_malformed_connect_urls() {
    let provider = Provider::start().await;
    let invalid = [
        format!("http://nyx.example/connect/{CONNECT_TOKEN}"),
        format!("https://attacker.example/connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example.evil/connect/{CONNECT_TOKEN}"),
        format!("https://user@nyx.example/connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example/not-connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}/extra"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}?next=elsewhere"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}#fragment"),
        "https://nyx.example/connect/nyx_clk_short".to_owned(),
    ];

    for connect_url in invalid {
        provider
            .state
            .set_create_reply(create_reply(connect_url.clone()));
        assert_eq!(
            broker(&provider, Tokens::available())
                .create_handoff(
                    &SubjectId::new("visitor-one"),
                    &ServiceId::new("github"),
                    "intent-one",
                )
                .await,
            Err(malformed_error()),
            "accepted unsafe connect URL {connect_url}"
        );
    }
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
        "expires_at": PROVIDER_EXPIRES_AT,
        "connected_service": { "id": "user-service-one", "slug": "visitor-github" }
    })));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": true,
                "credential_source": { "type": "personal" }
            }
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
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": false,
                "credential_source": { "type": "personal" }
            }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("inactive completion resolves"),
        BrokerHandoffState::Failed
    );

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-one",
                    "org_name": "Read-only org",
                    "role": "viewer",
                    "allowed": false
                }
            }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("denied completion resolves"),
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

    for status in [
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_EARLY,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        provider.state.set_create_reply(Reply::error(
            status,
            r#"{"message":"private retry detail"}"#,
        ));
        let error = broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await
            .expect_err("retryable provider failure is unavailable");
        assert_eq!(
            error,
            ServiceConnectionError::Unavailable("NyxID request failed".into())
        );
        assert!(!error.to_string().contains("private retry detail"));
    }

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

#[tokio::test]
async fn malformed_timestamps_and_provider_response_limits_fail_closed() {
    let provider = Provider::start().await;
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_create_reply(Reply::json(json!({
        "id": "link-one",
        "connect_url": trusted_connect_url(),
        "expires_at": "not-a-timestamp",
    })));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(malformed_error())
    );

    provider
        .state
        .set_create_reply(Reply::error(StatusCode::OK, &"x".repeat(1024 * 1024 + 1)));
    assert_eq!(
        broker(&provider, Tokens::available())
            .create_handoff(&owner, &service, "intent-one")
            .await,
        Err(malformed_error())
    );

    let rows = (0..=1_000)
        .map(|index| {
            json!({
                "id": format!("service-{index}"),
                "slug": format!("service-{index}"),
                "is_active": true,
                "credential_source": { "type": "personal" }
            })
        })
        .collect::<Vec<_>>();
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": rows })));
    assert_eq!(
        broker(&provider, Tokens::available())
            .requirements(&owner, std::slice::from_ref(&service))
            .await,
        Err(malformed_error())
    );
}
