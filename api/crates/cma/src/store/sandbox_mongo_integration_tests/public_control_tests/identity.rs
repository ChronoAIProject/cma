//! A loopback provider exercises attach_subject without injected caller extensions.
use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

pub(super) struct Identity {
    pub auth: Arc<crate::auth::AuthState>,
    pub owner: String,
    pub foreign: String,
    pub denied: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Identity {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Identity {
    pub async fn new() -> Self {
        let owner = uuid::Uuid::new_v4().to_string();
        let foreign = uuid::Uuid::new_v4().to_string();
        let denied = uuid::Uuid::new_v4().to_string();
        let replies = Arc::new(HashMap::from([
            (
                owner.clone(),
                json!({"sub":"owner", "permissions":["cma:sandbox:use"]}),
            ),
            (
                foreign.clone(),
                json!({"sub":"foreign", "permissions":["cma:sandbox:use"]}),
            ),
            (denied.clone(), json!({"sub":"owner", "permissions":[]})),
        ]));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "userinfo_endpoint": format!("{base}/userinfo"),
        });
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(move || async move { Json(metadata) }))
            .route("/userinfo", get(|State(replies): State<Arc<HashMap<String, Value>>>, headers: HeaderMap| async move {
                let token = headers.get("authorization").and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer ")).unwrap_or_default();
                Json(replies.get(token).expect("only fixture credentials reach the provider").clone())
            }))
            .with_state(replies);
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Provider {
                base_url: base,
                client_id: "admission-fixture".into(),
                client_secret: uuid::Uuid::new_v4().to_string(),
                public_url: "http://localhost".into(),
            },
            None,
        );
        Self {
            auth,
            owner,
            foreign,
            denied,
            server,
        }
    }
}
