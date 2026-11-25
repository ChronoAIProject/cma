use super::server::Server;
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::RwLock;

mod oauth;

#[derive(Clone)]
struct Identity {
    subject: String,
    active: bool,
    allowed: bool,
}

struct StateData {
    identities: RwLock<HashMap<String, Identity>>,
    grants: RwLock<HashMap<String, oauth::Grant>>,
    exchanges: AtomicUsize,
    client_secret: String,
    calls: AtomicUsize,
}

pub(super) struct Provider {
    pub server: Server,
    state: Arc<StateData>,
}

impl Provider {
    pub async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let discovery = json!({
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "userinfo_endpoint": format!("{base}/userinfo"),
            "introspection_endpoint": format!("{base}/introspect"),
        });
        let state = Arc::new(StateData {
            identities: RwLock::new(HashMap::new()),
            grants: RwLock::new(HashMap::new()),
            exchanges: AtomicUsize::new(0),
            client_secret: uuid::Uuid::new_v4().to_string(),
            calls: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async move { Json(discovery) }),
            )
            .route("/authorize", get(oauth::authorize))
            .route("/token", post(oauth::token))
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspect))
            .with_state(state.clone());
        Self {
            server: Server::serve(listener, app),
            state,
        }
    }

    pub fn mode(&self) -> crate::auth::AuthMode {
        crate::auth::AuthMode::Provider {
            base_url: self.server.base.clone(),
            client_id: "revocation-contract".into(),
            client_secret: self.state.client_secret.clone(),
            public_url: "https://cma.example.test".into(),
        }
    }

    pub async fn register(&self, subject: &str) -> String {
        let token = uuid::Uuid::new_v4().to_string();
        self.state.identities.write().await.insert(
            token.clone(),
            Identity {
                subject: subject.into(),
                active: true,
                allowed: true,
            },
        );
        token
    }

    pub async fn authority(&self, subject: &str, active: bool, allowed: bool) {
        for identity in self.state.identities.write().await.values_mut() {
            if identity.subject == subject {
                identity.active = active;
                identity.allowed = allowed;
            }
        }
    }

    pub fn calls(&self) -> usize {
        self.state.calls.load(Ordering::SeqCst)
    }

    pub fn exchanges(&self) -> usize {
        self.state.exchanges.load(Ordering::SeqCst)
    }
}

async fn userinfo(State(state): State<Arc<StateData>>, headers: HeaderMap) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    let identities = state.identities.read().await;
    match identities.get(token).filter(|identity| identity.active) {
        Some(identity) => Json(json!({"sub":identity.subject})).into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

async fn introspect(
    State(state): State<Arc<StateData>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert!(form
        .get("client_id")
        .is_some_and(|v| v == "revocation-contract"));
    assert!(form
        .get("client_secret")
        .is_some_and(|v| v == &state.client_secret));
    let identities = state.identities.read().await;
    match form.get("token").and_then(|token| identities.get(token)) {
        Some(identity) => Json(json!({
            "active":identity.active, "sub":identity.subject,
            "permissions":if identity.allowed {vec!["cma:sandbox:use"]} else {vec![]},
        }))
        .into_response(),
        None => Json(json!({"active":false})).into_response(),
    }
}
