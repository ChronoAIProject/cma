use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::store::InMemorySandboxStore;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

use crate::auth::{AuthMode, AuthState, ProviderMetadata};

mod capture;

pub(super) struct Reply {
    pub status: StatusCode,
    pub body: String,
}

impl Reply {
    pub fn json(status: StatusCode, body: Value) -> Self {
        Self {
            status,
            body: body.to_string(),
        }
    }
}

pub(super) struct Control {
    pub inventory: RwLock<Reply>,
    pub introspection: RwLock<Value>,
    pub inventory_calls: AtomicUsize,
    pub userinfo_calls: AtomicUsize,
    pub introspection_calls: AtomicUsize,
    pub revoke_on_introspection: AtomicUsize,
}

async fn userinfo(State(control): State<Arc<Control>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture-credential");
    control.userinfo_calls.fetch_add(1, Ordering::SeqCst);
    Json(json!({"sub":"human","permissions":["cma:sandbox:use"]}))
}

async fn introspection(State(control): State<Arc<Control>>) -> Json<Value> {
    let attempt = control.introspection_calls.fetch_add(1, Ordering::SeqCst) + 1;
    let revoke_on = control.revoke_on_introspection.load(Ordering::SeqCst);
    if revoke_on > 0 && attempt >= revoke_on {
        return Json(json!({"active":false}));
    }
    Json(control.introspection.read().await.clone())
}

async fn inventory(State(control): State<Arc<Control>>, headers: HeaderMap) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture-credential");
    // Reaching the org route proves this response follows successful identity
    // verification, rather than an authentication-middleware rejection.
    assert!(control.userinfo_calls.load(Ordering::SeqCst) > 0);
    assert!(control.introspection_calls.load(Ordering::SeqCst) > 0);
    control.inventory_calls.fetch_add(1, Ordering::SeqCst);
    let reply = control.inventory.read().await;
    (
        reply.status,
        [(header::CONTENT_TYPE, "application/json")],
        reply.body.clone(),
    )
        .into_response()
}

pub(super) struct Fixture {
    pub control: Arc<Control>,
    pub apps: [Router; 2],
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    pub async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let control = Arc::new(Control {
            inventory: RwLock::new(Reply::json(
                StatusCode::OK,
                json!({"orgs":[{"id":"org-a","your_role":"admin"}]}),
            )),
            introspection: RwLock::new(
                json!({"active":true,"sub":"human","permissions":["cma:sandbox:use"]}),
            ),
            inventory_calls: AtomicUsize::new(0),
            userinfo_calls: AtomicUsize::new(0),
            introspection_calls: AtomicUsize::new(0),
            revoke_on_introspection: AtomicUsize::new(0),
        });
        let provider = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspection))
            .route("/api/v1/orgs", get(inventory))
            .with_state(control.clone());
        let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
        // Separate AuthStates model independent managers: neither process may
        // serve another manager's completed identity or organization snapshot.
        let apps = [
            Self::application(&base).await,
            Self::application(&base).await,
        ];
        Self {
            control,
            apps,
            server,
        }
    }

    async fn application(base: &str) -> Router {
        let auth = AuthState::in_memory(
            AuthMode::Provider {
                base_url: base.into(),
                client_id: "fixture".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://cma.test".into(),
            },
            None,
        );
        *auth.metadata.write().await = Some(ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: Some(format!("{base}/introspect")),
        });
        let access =
            ResourceAccessService::new(Arc::new(InMemorySandboxStore::default()), auth.clone());
        layer1_public::rest::resource_access_router(Arc::new(access)).layer(
            axum::middleware::from_fn_with_state(auth, crate::auth::attach_subject),
        )
    }

    pub async fn request(&self, manager: usize) -> (StatusCode, Value) {
        let (status, body, _) = self.observed_request(manager).await;
        (status, body)
    }

    pub async fn observed_request(&self, manager: usize) -> (StatusCode, Value, Vec<Value>) {
        let ((status, body), events) = capture::run(self.request_inner(manager)).await;
        (status, body, events)
    }

    async fn request_inner(&self, manager: usize) -> (StatusCode, Value) {
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            self.apps[manager].clone().oneshot(
                Request::get("/api/v1/organizations")
                    .header(header::AUTHORIZATION, "Bearer fixture-credential")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("bounded local organization request")
        .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        for private in ["fixture-credential", "fixture-secret", "must-not-leak"] {
            assert!(
                !text.contains(private),
                "provider detail escaped into HTTP response"
            );
        }
        (status, serde_json::from_str(&text).unwrap())
    }

    pub async fn reply(&self, status: StatusCode, body: Value) {
        *self.control.inventory.write().await = Reply::json(status, body);
    }

    pub fn calls(&self) -> usize {
        self.control.inventory_calls.load(Ordering::SeqCst)
    }

    pub async fn close(mut self) {
        self.server.abort();
        let result = (&mut self.server).await;
        assert!(matches!(result, Err(error) if error.is_cancelled()));
    }
}
