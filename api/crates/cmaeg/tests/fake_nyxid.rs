use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::oneshot};

pub struct FakeNyxid {
    pub base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
}

impl FakeNyxid {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, signal) = oneshot::channel();
        let router = Router::new()
            .route("/api/v1/users/me", get(users_me))
            .with_state(Arc::new(()));
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
            shutdown: Some(shutdown),
        }
    }

    pub async fn close(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn users_me(headers: HeaderMap, _state: State<Arc<()>>) -> impl IntoResponse {
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
