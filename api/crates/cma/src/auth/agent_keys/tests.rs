use super::*;
use crate::auth::persistence::{AuthStore, MemoryAuthStore};
use axum::{
    http::{header, HeaderMap, StatusCode},
    routing::post,
    Form, Json, Router,
};
use layer2_core::sandbox::InMemorySandboxStore;
use layer2_core::{Capability, SubjectId};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::RwLock;

mod http;
mod security;

struct Fixture {
    base: String,
    response: Arc<RwLock<Value>>,
    status: Arc<RwLock<StatusCode>>,
    response_headers: Arc<RwLock<HeaderMap>>,
    introspections: Arc<AtomicUsize>,
    bindings: Arc<InMemorySandboxStore>,
    store: Arc<MemoryAuthStore>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let response = Arc::new(RwLock::new(json!({
            "active":true, "sub":"owner", "token_type":"api_key",
            "aud":"cma-service", "scope":"proxy",
            "permissions":["cma:sandbox:use", "cma:profile:manage"]
        })));
        let control = response.clone();
        let status = Arc::new(RwLock::new(StatusCode::OK));
        let controlled_status = status.clone();
        let response_headers = Arc::new(RwLock::new(HeaderMap::new()));
        let controlled_headers = response_headers.clone();
        let introspections = Arc::new(AtomicUsize::new(0));
        let observed = introspections.clone();
        let app = Router::new().route(
            "/introspect",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let control = control.clone();
                let status = controlled_status.clone();
                let observed = observed.clone();
                let response_headers = controlled_headers.clone();
                async move {
                    assert_eq!(form["service_id"], "cma-service");
                    assert_eq!(form["token_type_hint"], "api_key");
                    assert_eq!(form["client_id"], "cma-client");
                    assert!(is_agent_key(&form["token"]));
                    observed.fetch_add(1, Ordering::SeqCst);
                    (
                        *status.read().await,
                        response_headers.read().await.clone(),
                        Json(control.read().await.clone()),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            response,
            status,
            response_headers,
            introspections,
            bindings: Arc::new(InMemorySandboxStore::default()),
            store: Arc::new(MemoryAuthStore::default()),
            server,
        }
    }
    async fn auth(&self) -> Arc<AuthState> {
        let auth = AuthState::with_store(
            AuthMode::Provider {
                base_url: self.base.clone(),
                client_id: "cma-client".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "https://cma.example".into(),
            },
            None,
            self.store.clone(),
        )
        .with_agent_key_service(Some("cma-service".into()))
        .with_binding_store(self.bindings.clone());
        *auth.fixture_metadata().write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{}/authorize", self.base),
            token_endpoint: format!("{}/token", self.base),
            introspection_endpoint: Some(format!("{}/introspect", self.base)),
            userinfo_endpoint: None,
        });
        auth
    }
}
fn headers(key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {key}").parse().unwrap(),
    );
    headers.insert(header::COOKIE, "cma_session=unrelated".parse().unwrap());
    headers
}

#[tokio::test]
async fn ordinary_keys_pass_local_binding_dispatch_without_browser_custody() {
    let f = Fixture::new().await;
    let auth = f.auth().await;
    f.store.set_unavailable(true);
    for prefix in ["nyx_", "nyxid_ag_"] {
        let key = format!("{prefix}{}", uuid::Uuid::new_v4().simple());
        let request = auth.resolve_request(&headers(&key)).await.unwrap().unwrap();
        assert_eq!(request.subject, SubjectId::new("owner"));
        assert!(request.authorization.allows(Capability::UseOwnSandboxes));
        assert!(!request.authorization.allows(Capability::ControlAnySandbox));
        assert!(request.binding.is_none());
        assert_eq!(request.delegated_credential.as_deref(), Some(key.as_str()));
    }
}

#[tokio::test]
async fn incomplete_wrong_audience_and_revoked_evidence_cannot_authenticate() {
    let f = Fixture::new().await;
    let auth = f.auth().await;
    let original = f.response.read().await.clone();
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    for (field, value) in [
        ("active", json!(false)),
        ("aud", json!("other-service")),
        ("sub", json!("")),
        ("token_type", json!("access")),
        ("scope", json!("read")),
        ("exp", json!(1)),
        ("exp", json!(null)),
        ("exp", json!("invalid")),
        ("permissions", json!(null)),
        ("permissions", json!([1])),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        *f.response.write().await = changed;
        assert!(
            !matches!(auth.resolve_request(&headers(&key)).await, Ok(Some(_))),
            "accepted {field}"
        );
    }
}

#[tokio::test]
async fn nonexpiring_key_command_custody_survives_restart_and_rechecks_revocation() {
    let f = Fixture::new().await;
    let auth = f.auth().await;
    let key = format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple());
    let request = auth.resolve_request(&headers(&key)).await.unwrap().unwrap();
    let context = auth.resource_request_context(&request);
    let authority = auth
        .independent_command_authority(&context, &key)
        .await
        .unwrap();
    assert!(authority.authority_id.ends_with(":none"));
    assert!(f.store.subject(&request.subject).await.unwrap().is_none());
    let restarted = f.auth().await;
    let custody = restarted
        .retained_api_custody(&request.subject, &authority)
        .await
        .unwrap();
    assert!(custody.expires_at.is_none());
    assert!(restarted.verify_api_custody(&custody).await.is_ok());
    *f.response.write().await = json!({"active":false});
    assert!(restarted.verify_api_custody(&custody).await.is_err());
}

#[tokio::test]
async fn expiring_key_custody_preserves_exact_expiry_and_rechecks_authority() {
    use layer2_core::sandbox::agent_io::CommandAuthorizationError;
    let fixture = Fixture::new().await;
    let expiry = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    fixture.response.write().await["exp"] = json!(expiry);
    let original = fixture.response.read().await.clone();
    let auth = fixture.auth().await;
    let key = format!("nyx_{}", uuid::Uuid::new_v4().simple());
    let request = auth.resolve_request(&headers(&key)).await.unwrap().unwrap();
    let authority = auth
        .independent_command_authority(&auth.resource_request_context(&request), &key)
        .await
        .unwrap();
    let restarted = fixture.auth().await;
    let custody = restarted
        .retained_api_custody(&request.subject, &authority)
        .await
        .unwrap();
    assert_eq!(
        custody.expires_at,
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(expiry))
    );
    assert!(restarted.verify_api_custody(&custody).await.is_ok());
    for (field, value) in [
        ("active", json!(false)),
        ("sub", json!("another-owner")),
        ("aud", json!("another-service")),
        ("permissions", json!([])),
        ("exp", json!(expiry + 1)),
        ("exp", json!(1)),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        *fixture.response.write().await = changed;
        assert!(
            matches!(
                restarted.verify_api_custody(&custody).await,
                Err(CommandAuthorizationError::Revoked)
            ),
            "accepted changed {field}"
        );
    }
    *fixture.response.write().await = original;
    *fixture.status.write().await = StatusCode::BAD_GATEWAY;
    assert!(matches!(
        restarted.verify_api_custody(&custody).await,
        Err(CommandAuthorizationError::Unavailable)
    ));
    assert!(fixture
        .store
        .subject(&request.subject)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn retained_ordinary_key_loses_owner_authority_when_registered_as_a_binding() {
    use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
    use layer2_core::sandbox::agent_io::CommandAuthorizationError;
    use layer2_core::sandbox::binding::{
        map_profile, BindingStore, ProvidedCredential, ProviderBinding,
    };

    let fixture = Fixture::new().await;
    let auth = fixture.auth().await;
    let key = ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let request = auth
        .resolve_request(&headers(key.expose()))
        .await
        .unwrap()
        .unwrap();
    let authority = auth
        .independent_command_authority(&auth.resource_request_context(&request), key.expose())
        .await
        .unwrap();
    let custody = auth
        .retained_api_custody(&request.subject, &authority)
        .await
        .unwrap();
    assert!(auth.verify_api_custody(&custody).await.is_ok());

    let profile = json!({
        "schema_version": 1,
        "metadata": {"name":"deferred custody", "owner":{"kind":"human", "subject":"owner"}, "provider":"registered"},
        "model": {"access":"managed", "service":AgentProvider::default().connection_services().managed,
            "api_type":"responses", "model":"test", "thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"}, "context":{},
        "environment":{"size":{"cpu_cores":1, "memory_gib":2}, "workspace":{"type":"directory", "name":"project"}}
    });
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    let mapped = map_profile(
        profile,
        &digest,
        AgentProvider::default(),
        "responses",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let binding = ProviderBinding::new(
        "binding-custody".into(),
        SubjectId::new("manager"),
        mapped,
        key.sha256(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    fixture.bindings.save_binding(None, binding).await.unwrap();
    let current = auth
        .resolve_request(&headers(key.expose()))
        .await
        .unwrap()
        .unwrap();
    assert!(current.binding.is_some());
    assert!(!current.authorization.allows(Capability::UseOwnSandboxes));
    let before = fixture.introspections.load(Ordering::SeqCst);
    assert!(matches!(
        auth.verify_api_custody(&custody).await,
        Err(CommandAuthorizationError::Revoked)
    ));
    assert_eq!(fixture.introspections.load(Ordering::SeqCst), before);
}
