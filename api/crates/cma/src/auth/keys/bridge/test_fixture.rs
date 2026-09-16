//! Local HTTP authority fixture. Its tokens are deliberately disposable.
use super::KeyIssuer;
mod reconciliation;
use crate::{
    auth::{
        bridge_control_plane::BridgeHumanLease,
        persistence::{DurableSubjectTokens, SecretValue, SubjectAuthState},
        profile::Profile,
    },
    bridge::{control_plane::BridgeControlPlaneGrant, BridgeControlPlaneLease},
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use layer2_core::{
    bridge::{BridgeConnector, BridgeId},
    resource_access::{CallerContext, CredentialContext, ResourceActor, ResourceOwner},
    Caller, SubjectId,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};

pub(crate) const HUMAN_SECRET: &str = "fixture-channel-human-access-secret";
pub(crate) const RUNTIME_SECRET: &str = "fixture-channel-runtime-key";

pub(crate) fn lease(
    owner: ResourceOwner,
    bridge_id: BridgeId,
    connector: BridgeConnector,
    revision: u64,
) -> BridgeControlPlaneLease {
    let subject = match &owner {
        ResourceOwner::Person { subject } => subject.clone(),
        _ => SubjectId::new("fixture-admin"),
    };
    let now = SystemTime::now();
    let state = SubjectAuthState::new(
        subject.clone(),
        crate::permissions::capabilities_from_permissions(["cma:*"]),
        Profile::default(),
        Some(DurableSubjectTokens {
            access_token: SecretValue::new(HUMAN_SECRET.into()),
            refresh_token: Some(SecretValue::new("fixture-refresh".into())),
            expires_at: now + Duration::from_secs(3600),
        }),
    );
    let context = CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: Caller::new(subject.clone(), state.authorization.clone()),
        credential: CredentialContext {
            issuer: connector.issuer.clone(),
            subject: subject.clone(),
            credential_id: "fixture-login".into(),
        },
        actor: ResourceActor::Api {
            subject: subject.clone(),
            delegation_id: None,
        },
        memberships: vec![],
    };
    let grant = BridgeControlPlaneGrant {
        bridge_id,
        owner,
        connector,
        subject,
        incarnation: state.incarnation.clone(),
        issued_at: layer2_core::rfc3339_at(now),
        expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(86400)),
    };
    BridgeControlPlaneLease::fixture(
        grant,
        BridgeHumanLease::fixture(
            state,
            context,
            tokio::time::Instant::now() + Duration::from_secs(60),
        ),
        revision,
    )
}

pub(crate) struct HttpState {
    pub owner: ResourceOwner,
    pub actor: SubjectId,
    pub mint_count: AtomicUsize,
    pub wrong_metadata_owner: AtomicBool,
    pub mint_unavailable: AtomicBool,
    pub create_body: Mutex<Option<Value>>,
    pub list_count: AtomicUsize,
    pub revoke_count: AtomicUsize,
    pub listing_override: Mutex<Option<Value>>,
    pub revoked: AtomicBool,
    pub delete_unknown: AtomicBool,
}
impl HttpState {
    fn owner(&self) -> Value {
        match &self.owner {
            ResourceOwner::Person { subject } => json!({"type":"personal","id":subject.as_str()}),
            ResourceOwner::Organization { subject } => {
                json!({"type":"organization","id":subject.as_str()})
            }
        }
    }
    fn source(&self) -> Value {
        if self.wrong_metadata_owner.load(Ordering::SeqCst) {
            return json!({"type":"org","org_id":"wrong-org","role":"admin","allowed":true});
        }
        match &self.owner {
            ResourceOwner::Person { .. } => json!({"type":"personal"}),
            ResourceOwner::Organization { subject } => {
                json!({"type":"org","org_id":subject.as_str(),"role":"admin","allowed":true})
            }
        }
    }
}
pub(crate) struct Server {
    pub issuer: Arc<KeyIssuer>,
    pub state: Arc<HttpState>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn new(lease: &BridgeControlPlaneLease, deployment: &str) -> Self {
        let state = Arc::new(HttpState {
            owner: lease.owner().clone(),
            actor: lease.actor_subject().clone(),
            mint_count: AtomicUsize::new(0),
            wrong_metadata_owner: AtomicBool::new(false),
            mint_unavailable: AtomicBool::new(false),
            create_body: Mutex::new(None),
            list_count: AtomicUsize::new(0),
            revoke_count: AtomicUsize::new(0),
            listing_override: Mutex::new(None),
            revoked: AtomicBool::new(false),
            delete_unknown: AtomicBool::new(false),
        });
        let router = Router::new()
            .route("/api/v1/user-services", get(services))
            .route("/api/v1/api-keys/scope-plan", post(scope))
            .route("/api/v1/api-keys", post(mint).get(reconciliation::list))
            .route(
                "/api/v1/api-keys/key-created",
                get(metadata).delete(reconciliation::revoke),
            )
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
        Self {
            issuer: Arc::new(KeyIssuer::new(http, base_url, deployment)),
            state,
            task,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
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
    let slug = layer3_adapter::AgentProvider::Codex
        .connection_services()
        .managed;
    Ok(Json(
        json!({"services":[{"id":"service-models","slug":slug,"is_active":true,"credential_source":state.source()},
        {"id":"service-personal-other","slug":slug,"is_active":true,"credential_source":{"type":"org","org_id":"another-owner","role":"admin","allowed":true}}]}),
    ))
}
async fn scope(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    assert_eq!(body["selected_service_ids"], json!(["service-models"]));
    match &state.owner {
        ResourceOwner::Person { .. } => assert!(body.get("target_org_id").is_none()),
        ResourceOwner::Organization { subject } => {
            assert_eq!(body["target_org_id"], subject.as_str())
        }
    }
    Ok(Json(
        json!({"authority":"nyxid","contract_version":"1","policy_version":"api-key-scope-v1",
        "authenticated_actor":{"type":"personal","id":state.actor.as_str()},"intended_key_owner":state.owner(),
        "services":[{"user_service_id":"service-models","resource_owner":state.owner(),"node_grant":{"type":"required","node_ids":["node-models"]}}],
        "allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],"normalized_grant_digest":"a".repeat(64),
        "completeness":{"list_complete":true,"no_duplicates":true,"route_candidate_basis":"active_configured_routes","transient_node_state_excluded":true},
        "freshness":{"mode":"mutation_revalidated_snapshot","precondition_field":"scope_plan_digest","post_creation_drift":"fail_closed"},"key_purpose":"general"}),
    ))
}
async fn mint(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    state.mint_count.fetch_add(1, Ordering::SeqCst);
    assert_eq!(body["allowed_service_ids"], json!(["service-models"]));
    assert_eq!(body["allowed_node_ids"], json!(["node-models"]));
    assert_eq!(body["allow_all_services"], false);
    assert_eq!(body["allow_all_nodes"], false);
    assert_eq!(body["scope_plan_digest"], "a".repeat(64));
    match &state.owner {
        ResourceOwner::Person { .. } => assert!(body.get("target_org_id").is_none()),
        ResourceOwner::Organization { subject } => {
            assert_eq!(body["target_org_id"], subject.as_str())
        }
    }
    *state.create_body.lock().unwrap() = Some(body);
    if state.mint_unavailable.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(
        json!({"id":"key-created","full_key":RUNTIME_SECRET,"scopes":"proxy read","state_version":1,
        "allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],"allow_all_services":false,"allow_all_nodes":false}),
    ))
}
async fn metadata(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    if state.revoked.load(Ordering::SeqCst) {
        return Err(StatusCode::NOT_FOUND);
    }
    let body = state
        .create_body
        .lock()
        .unwrap()
        .clone()
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(
        json!({"id":"key-created","name":body["name"],"expires_at":body["expires_at"],"is_active":true,
        "scopes":"proxy read","state_version":1,"allowed_service_ids":["service-models"],"allowed_node_ids":["node-models"],
        "allow_all_services":false,"allow_all_nodes":false,"credential_source":state.source(),"purpose":"general","scheduled_write_enabled":false}),
    ))
}
