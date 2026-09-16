use super::super::fixture::{Fixture, VerifiedHuman};
use crate::{
    auth::keys::{bridge::test_fixture::HUMAN_SECRET, KeyIssuer},
    sandbox::{
        channel_renewal::{model::ChannelRenewalSnapshot, ChannelCredentialRenewalDriver},
        SandboxCredentialCustody,
    },
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{bridge::allocation::AllocationError, resource_access::ResourceOwner};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

pub(crate) struct RenewalFixture {
    pub original: Fixture,
    pub driver: ChannelCredentialRenewalDriver,
    pub state: Arc<HttpState>,
    pub due: SystemTime,
    task: tokio::task::JoinHandle<()>,
}
pub(crate) struct HttpState {
    pub mint_count: AtomicUsize,
    pub revoke_count: AtomicUsize,
    pub mint_unknown: AtomicBool,
    pub keys: Mutex<BTreeMap<String, Value>>,
    pub scopes_unavailable: AtomicBool,
    pub list_override: Mutex<Option<Value>>,
    pub revoke_unknown: AtomicBool,
    source: Value,
    owner: Value,
    actor: String,
}
impl RenewalFixture {
    pub async fn new() -> Self {
        Self::from_original(Fixture::new().await).await
    }
    pub async fn from_original(mut original: Fixture) -> Self {
        original.ready().await;
        let material = original
            .custody
            .get(&original.record.placement.target().sandbox_id, 1)
            .await
            .unwrap()
            .unwrap();
        let body = original
            .server
            .state
            .create_body
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        let (owner, source) = match original.lease.owner() {
            ResourceOwner::Person { subject } => (
                json!({"type":"personal","id":subject.as_str()}),
                json!({"type":"personal"}),
            ),
            ResourceOwner::Organization { subject } => (
                json!({"type":"organization","id":subject.as_str()}),
                json!({"type":"org","org_id":subject.as_str(),"role":"admin","allowed":true}),
            ),
        };
        let mut keys = BTreeMap::new();
        keys.insert(
            material.authority.provider_key_id.clone(),
            key_metadata(
                &material.authority.provider_key_id,
                &body,
                source.clone(),
                material.authority.provider_state_version,
            ),
        );
        let state = Arc::new(HttpState {
            mint_count: AtomicUsize::new(0),
            revoke_count: AtomicUsize::new(0),
            mint_unknown: AtomicBool::new(false),
            keys: Mutex::new(keys),
            scopes_unavailable: AtomicBool::new(false),
            list_override: Mutex::new(None),
            revoke_unknown: AtomicBool::new(false),
            owner,
            source,
            actor: original.lease.actor_subject().to_string(),
        });
        let router = Router::new()
            .route("/api/v1/user-services", get(services))
            .route("/api/v1/api-keys/scope-plan", post(scope))
            .route("/api/v1/api-keys", post(mint).get(list))
            .route("/api/v1/api-keys/{id}", get(metadata).delete(revoke))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let driver = ChannelCredentialRenewalDriver {
            allocations: original.allocations.clone(),
            credentials: Arc::new(VerifiedHuman),
            issuer: Arc::new(KeyIssuer::new(http, base_url, "fixture-deployment")),
            custody: original.custody.clone(),
            adapter: original.adapter.clone(),
            holder: "channel-renewal-one".into(),
        };
        Self {
            original,
            driver,
            state,
            due: SystemTime::now() + Duration::from_secs(13 * 60 * 60),
            task,
        }
    }
    pub async fn step(&self) -> Result<(), AllocationError> {
        self.driver
            .run_one(
                &self.original.record,
                &self.original.authority,
                &self.original.lease,
                self.due,
            )
            .await
    }
    pub async fn snapshot(&self) -> ChannelRenewalSnapshot {
        self.original
            .allocations
            .current_channel_renewal(
                &self.original.authority,
                &self.original.lease,
                &self.original.record,
            )
            .await
            .unwrap()
    }
    pub async fn close(self) {
        self.task.abort();
        self.original.close().await;
    }
}
fn authorized(headers: &HeaderMap) -> Result<(), StatusCode> {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&format!("Bearer {HUMAN_SECRET}"))
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(())
}
async fn services(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    if state.scopes_unavailable.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(
        json!({"services":[{"id":"service-models","slug":layer3_adapter::AgentProvider::Codex.connection_services().managed,
        "is_active":true,"credential_source":state.source}]}),
    ))
}
async fn scope(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    assert_eq!(body["selected_service_ids"], json!(["service-models"]));
    require_owner(&state, &body);
    Ok(Json(
        json!({"authority":"nyxid","contract_version":"1","policy_version":"api-key-scope-v1",
        "authenticated_actor":{"type":"personal","id":state.actor},"intended_key_owner":state.owner,
        "services":[{"user_service_id":"service-models","resource_owner":state.owner,"node_grant":{"type":"required","node_ids":["node-models"]}}],
        "allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],"normalized_grant_digest":"a".repeat(64),
        "completeness":{"list_complete":true,"no_duplicates":true,"route_candidate_basis":"active_configured_routes","transient_node_state_excluded":true},
        "freshness":{"mode":"mutation_revalidated_snapshot","precondition_field":"scope_plan_digest","post_creation_drift":"fail_closed"},"key_purpose":"general"}),
    ))
}
fn require_owner(state: &HttpState, body: &Value) {
    if state.owner["type"] == "organization" {
        assert_eq!(body["target_org_id"], state.owner["id"]);
    } else {
        assert!(body.get("target_org_id").is_none());
    }
}
fn key_metadata(id: &str, body: &Value, source: Value, state_version: i64) -> Value {
    json!({"id":id,"name":body["name"],"expires_at":body["expires_at"],"is_active":true,"created_at":layer2_core::rfc3339_at(SystemTime::now()),
        "scopes":"proxy read","state_version":state_version,"allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],
        "allow_all_services":false,"allow_all_nodes":false,"credential_source":source,"purpose":"general","scheduled_write_enabled":false})
}
async fn mint(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    require_owner(&state, &body);
    assert_eq!(body["allowed_service_ids"], json!(["service-models"]));
    assert_eq!(body["allowed_node_ids"], json!(["node-models"]));
    let number = state.mint_count.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("key-renewed-{number}");
    state.keys.lock().unwrap().insert(
        id.clone(),
        key_metadata(&id, &body, state.source.clone(), 1),
    );
    if state.mint_unknown.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(
        json!({"id":id,"full_key":format!("fixture-runtime-renewed-{number}"),"scopes":"proxy read","state_version":1,
        "allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],"allow_all_services":false,"allow_all_nodes":false}),
    ))
}
async fn metadata(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    state
        .keys
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}
async fn revoke(
    State(state): State<Arc<HttpState>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    authorized(&headers)?;
    let previous = state.keys.lock().unwrap().remove(&id);
    if previous.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    state.revoke_count.fetch_add(1, Ordering::SeqCst);
    if state.revoke_unknown.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    match state.owner["type"].as_str() {
        Some("personal") => assert!(!query.contains_key("org_id")),
        Some("organization") => assert_eq!(
            query.get("org_id").map(String::as_str),
            state.owner["id"].as_str()
        ),
        _ => unreachable!(),
    }
    if let Some(value) = state.list_override.lock().unwrap().clone() {
        return Ok(Json(value));
    }
    Ok(Json(
        json!({"keys":state.keys.lock().unwrap().values().cloned().collect::<Vec<_>>()}),
    ))
}
