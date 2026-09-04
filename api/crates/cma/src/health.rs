//! Liveness and dependency-aware readiness endpoints.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use layer2_core::StoreError;

use crate::store::MongoStores;

#[async_trait::async_trait]
pub trait ReadinessProbe: Send + Sync {
    async fn ping(&self) -> Result<(), StoreError>;
}

#[async_trait::async_trait]
impl ReadinessProbe for MongoStores {
    async fn ping(&self) -> Result<(), StoreError> {
        MongoStores::ping(self).await
    }
}

pub fn router(probe: Arc<dyn ReadinessProbe>) -> Router {
    Router::new()
        .route("/livez", get(liveness))
        .route("/ready", get(readiness))
        .with_state(probe)
}

async fn liveness() -> &'static str {
    "ok"
}

async fn readiness(State(probe): State<Arc<dyn ReadinessProbe>>) -> impl IntoResponse {
    match probe.ping().await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "ready" })),
        ),
        Err(error) => {
            tracing::warn!(error = %error, "readiness dependency did not answer");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "status": "not-ready",
                    "reason": "store",
                })),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::{body::Body, http::Request};
    use tower::ServiceExt as _;

    use super::*;

    struct Probe(AtomicBool);

    #[async_trait::async_trait]
    impl ReadinessProbe for Probe {
        async fn ping(&self) -> Result<(), StoreError> {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(StoreError::Unavailable("offline".into()))
            }
        }
    }

    async fn status(app: &Router, path: &str) -> StatusCode {
        app.clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn dependency_failure_affects_readiness_but_not_liveness() {
        let probe = Arc::new(Probe(AtomicBool::new(false)));
        let app = router(probe.clone());
        assert_eq!(
            status(&app, "/ready").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(status(&app, "/livez").await, StatusCode::OK);
        probe.0.store(true, Ordering::SeqCst);
        assert_eq!(status(&app, "/ready").await, StatusCode::OK);
    }
}
