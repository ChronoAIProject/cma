use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode};
use axum::{
    http::{header, HeaderMap, StatusCode},
    Router,
};
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::{
    map_profile, BindingState, BindingWork, ProvidedCredential, ProviderBinding,
};
use layer2_core::sandbox::InMemorySandboxStore;
use layer2_core::{Capability, SubjectId};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

pub(super) fn reserved(key: &ProvidedCredential) -> ProviderBinding {
    reserved_with_owner(key, "human")
}

pub(super) fn reserved_with_owner(key: &ProvidedCredential, owner_kind: &str) -> ProviderBinding {
    let profile = json!({"schema_version":1,"metadata":{"name":"authentication","owner":{"kind":owner_kind,"subject":"owner"},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
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
    ProviderBinding::new(
        "binding-auth".into(),
        SubjectId::new("manager"),
        mapped,
        key.sha256(),
        layer2_core::now_rfc3339(),
    )
    .unwrap()
}
fn headers(key: &ProvidedCredential) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", key.expose()).parse().unwrap(),
    );
    headers.insert(
        header::COOKIE,
        "cma_session=unavailable-cookie".parse().unwrap(),
    );
    headers
}
pub(super) fn key() -> ProvidedCredential {
    ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap()
}

struct DeletingBindingOwner(bool);
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for DeletingBindingOwner {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(self.0)
    }
}

#[tokio::test]
async fn binding_authentication_checks_standing_and_refuses_owner_deletion_authority() {
    for (erasing, path, code) in [
        (true, "/api/v2/responses", "account_deletion_in_progress"),
        (
            false,
            "/api/v1/account/deletion",
            "account_deletion_requires_owner_session",
        ),
    ] {
        let store = Arc::new(InMemorySandboxStore::default());
        let credential = key();
        store
            .save_binding(None, reserved(&credential))
            .await
            .unwrap();
        let auth = AuthState::in_memory(
            AuthMode::Trusted {
                subject: "development".into(),
            },
            None,
        )
        .with_binding_store(store)
        .with_account_standing(Arc::new(DeletingBindingOwner(erasing)));
        let app = Router::new()
            .route(
                path,
                axum::routing::post(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::attach_subject,
            ));
        let mut request = axum::http::Request::post(path)
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = headers(&credential);
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], code);
    }
}

#[tokio::test]
async fn account_deletion_endpoint_refuses_binding_before_scope_and_marks_private() {
    use layer2_core::account_deletion::{
        AccountDeletionService, AvailableAccounts, MemoryAccountDeletionStore,
    };
    let store = Arc::new(InMemorySandboxStore::default());
    let credential = key();
    store
        .save_binding(None, reserved(&credential))
        .await
        .unwrap();
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "development".into(),
        },
        None,
    )
    .with_binding_store(store);
    let service = Arc::new(AccountDeletionService::new(
        Arc::new(MemoryAccountDeletionStore::default()),
        Arc::new(AvailableAccounts),
        true,
    ));
    let app = layer1_public::rest::account_deletion_router(service).layer(
        axum::middleware::from_fn_with_state(auth, crate::auth::attach_subject),
    );
    for method in ["GET", "POST"] {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri("/api/v1/account/deletion")
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = headers(&credential);
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], "account_deletion_requires_owner_session");
        assert_eq!(
            body["type"],
            "https://spec.chrono-ai.fun/cma/1.0/problems/account-deletion-requires-owner-session"
        );
    }
}

#[tokio::test]
async fn binding_key_bypasses_nyxid_and_cookie_custody_and_never_inherits_owner_rights() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().fallback(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }),
        )
        .await
        .unwrap();
    });
    let auth_store = Arc::new(MemoryAuthStore::default());
    auth_store.set_unavailable(true);
    let store = Arc::new(InMemorySandboxStore::default());
    let auth = AuthState::with_store(
        AuthMode::Provider {
            base_url: base,
            client_id: "client".into(),
            client_secret: uuid::Uuid::new_v4().to_string(),
            public_url: "https://cma.example".into(),
        },
        None,
        auth_store,
    )
    .with_agent_key_service(Some("cma-service".into()))
    .with_binding_store(store.clone());
    let original = key();
    let mut binding = reserved(&original);
    store.save_binding(None, binding.clone()).await.unwrap();
    let resolved = auth
        .resolve_request(&headers(&original))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.subject, binding.owner);
    assert_eq!(
        resolved.authorization.iter().collect::<Vec<_>>(),
        vec![Capability::UseBindingConversations]
    );
    assert_eq!(
        resolved.binding.as_ref().unwrap().binding_id(),
        binding.binding_id
    );
    let context = auth.resource_request_context(&resolved);
    assert!(context.validate_principal());
    assert_eq!(context.credential_source, CredentialSource::Binding);
    assert_eq!(context.credential.credential_id, original.sha256());
    async fn inspect(
        axum::Extension(method): axum::Extension<layer1_public::AuthMethod>,
    ) -> StatusCode {
        assert_eq!(
            method,
            layer1_public::AuthMethod::BindingKey {
                binding_id: "binding-auth".into()
            }
        );
        StatusCode::NO_CONTENT
    }
    let app = Router::new()
        .route("/api/v2/responses", axum::routing::post(inspect))
        .route(
            "/api/v1/agents",
            axum::routing::get(|| async { StatusCode::OK }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth.clone(),
            crate::auth::attach_subject,
        ));
    for (method, path, expected) in [
        ("POST", "/api/v2/responses", StatusCode::NO_CONTENT),
        ("GET", "/api/v1/agents", StatusCode::FORBIDDEN),
    ] {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .body(axum::body::Body::empty())
            .unwrap();
        *request.headers_mut() = headers(&original);
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // Only an unknown key may reach the ordinary service verifier.
    assert!(auth.resolve_request(&headers(&key())).await.is_err());
    let unknown_calls = calls.load(Ordering::SeqCst);
    assert!(unknown_calls > 0);
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store.save_binding(Some(1), binding.clone()).await.unwrap();
    let replacement = key();
    let rotated = binding
        .rotate_credential(replacement.sha256(), layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(binding.revision), rotated.clone())
        .await
        .unwrap();
    assert!(auth
        .resolve_request(&headers(&original))
        .await
        .unwrap()
        .is_none());
    assert!(auth
        .resolve_request(&headers(&replacement))
        .await
        .unwrap()
        .is_some());
    let mut retired = rotated.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    store
        .save_binding(Some(rotated.revision), retired)
        .await
        .unwrap();
    assert!(auth
        .resolve_request(&headers(&replacement))
        .await
        .unwrap()
        .is_none());
    assert_eq!(calls.load(Ordering::SeqCst), unknown_calls);
    server.abort();
}

#[test]
fn binding_request_paths_exclude_management_and_foreign_event_context() {
    use axum::http::Method;
    assert!(permits_request(
        &Method::POST,
        "/api/v1/bot-setups/actions",
        "binding-auth"
    ));
    assert!(!permits_request(
        &Method::GET,
        "/api/v1/bot-setups/actions",
        "binding-auth"
    ));
    assert!(permits_request(
        &Method::POST,
        "/api/v1/bot-setups/advance",
        "binding-auth"
    ));
    assert!(!permits_request(
        &Method::GET,
        "/api/v1/bot-setups/advance",
        "binding-auth"
    ));
    assert!(!permits_request(
        &Method::POST,
        "/api/v1/bots",
        "binding-auth"
    ));
    for path in [
        "/api/v1/agents",
        "/api/v1/sandboxes",
        "/api/v1/profiles",
        "/auth/session",
        "/api/v1/cmaeg/worker-credentials/classify",
        "/api/v1/cmaeg/bindings/binding-auth",
        "/api/v1/cmaeg/bindings/other/conversations/conversation/events/event",
    ] {
        assert!(
            !permits_request(&Method::GET, path, "binding-auth"),
            "{path}"
        );
    }
    assert!(permits_request(
        &Method::GET,
        "/api/v1/cmaeg/bindings/binding-auth/conversations/conversation/events/event",
        "binding-auth"
    ));
    assert!(!permits_request(
        &Method::DELETE,
        "/api/v1/cmaeg/bindings/binding-auth/conversations/conversation",
        "binding-auth"
    ));
}
