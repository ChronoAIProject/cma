use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Form, Json, Router,
};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use tokio::{net::TcpListener, sync::oneshot};

pub struct FakeNyxid {
    pub base_url: String,
    pub service_tokens: Arc<TokenState>,
    shutdown: Option<oneshot::Sender<()>>,
}

pub struct TokenState {
    issuer: String,
    pub calls: AtomicUsize,
    pub expires_in: AtomicU64,
}

impl FakeNyxid {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, signal) = oneshot::channel();
        let service_tokens = Arc::new(TokenState {
            issuer: format!("http://{address}"),
            calls: AtomicUsize::new(0),
            expires_in: AtomicU64::new(3600),
        });
        let router = Router::new()
            .route("/api/v1/users/me", get(users_me))
            .route("/oauth/token", post(service_token))
            .route("/oauth/introspect", post(introspect))
            .with_state(service_tokens.clone());
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = signal.await;
                })
                .await
                .unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            service_tokens,
            shutdown: Some(shutdown),
        }
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn users_me(headers: HeaderMap) -> impl IntoResponse {
    let Some(value) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
    else {
        return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response();
    };
    match value {
        "Bearer token-alice" => Json(serde_json::json!({"subject":"alice"})).into_response(),
        "Bearer token-bob" => Json(serde_json::json!({"subject":"bob"})).into_response(),
        _ => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({}))).into_response(),
    }
}

async fn service_token(
    State(state): State<Arc<TokenState>>,
    Form(form): Form<std::collections::BTreeMap<String, String>>,
) -> impl IntoResponse {
    assert_eq!(form["client_id"], "fixture-client");
    assert_eq!(form["client_secret"], "fixture-client-secret");
    assert_eq!(form["grant_type"], "client_credentials");
    assert_eq!(form["scope"], "openid roles");
    state.calls.fetch_add(1, Ordering::SeqCst);
    Json(
        serde_json::json!({"access_token":"cma-service", "token_type":"Bearer",
        "expires_in":state.expires_in.load(Ordering::SeqCst)}),
    )
}

async fn introspect(
    State(state): State<Arc<TokenState>>,
    Form(form): Form<std::collections::BTreeMap<String, String>>,
) -> impl IntoResponse {
    assert_eq!(form["client_id"], "gateway-management");
    assert_eq!(form["client_secret"], "gateway-management-fixture");
    let subject = match form.get("token").map(String::as_str) {
        Some("token-alice") => "alice",
        Some("token-bob") => "bob",
        _ => return Json(serde_json::json!({"active":false})),
    };
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    Json(
        serde_json::json!({"active":true,"sub":subject,"iss":state.issuer,"exp":expiry,
        "token_type":"access","resource":[format!("{}/api/v1/proxy/s/cma-event-gateway",state.issuer)],
        "permissions":["cma-eg:admin"]}),
    )
}
