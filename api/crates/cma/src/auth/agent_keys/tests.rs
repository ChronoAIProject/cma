use super::*;
use crate::auth::persistence::{AuthStore, MemoryAuthStore};
use axum::{
    http::{header, HeaderMap},
    routing::post,
    Form, Json, Router,
};
use layer2_core::sandbox::InMemorySandboxStore;
use layer2_core::{Capability, SubjectId};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

struct Fixture {
    base: String,
    response: Arc<RwLock<Value>>,
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
        let app = Router::new().route(
            "/introspect",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let control = control.clone();
                async move {
                    assert_eq!(form["service_id"], "cma-service");
                    assert_eq!(form["token_type_hint"], "api_key");
                    assert_eq!(form["client_id"], "cma-client");
                    assert!(is_agent_key(&form["token"]));
                    Json(control.read().await.clone())
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
        .with_binding_store(Arc::new(InMemorySandboxStore::default()));
        *auth.metadata.write().await = Some(ProviderMetadata {
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
