//! Process health endpoints.

use std::sync::Arc;

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use layer2_core::SessionStore;
use serde::Serialize;

#[derive(Serialize)]
struct Readiness {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

pub(crate) fn router(store: Arc<dyn SessionStore>) -> Router {
    Router::new()
        // Liveness is process-only. Restarting this process cannot repair its
        // database, and would discard the live runtime links it can preserve.
        .route("/livez", get(|| async { "ok" }))
        .route("/ready", get(move || ready(Arc::clone(&store))))
}

async fn ready(store: Arc<dyn SessionStore>) -> impl IntoResponse {
    // Only the session store gates readiness. NyxID and the sandbox provider
    // degrade specific operations when unavailable, while reads and existing
    // sessions still work; removing the pod from service would hide those too.
    match store.ping().await {
        Ok(()) => (
            StatusCode::OK,
            Json(Readiness {
                status: "ready",
                reason: None,
            }),
        ),
        Err(error) => {
            tracing::warn!(error = %error, "session store readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Readiness {
                    status: "not-ready",
                    reason: Some("store"),
                }),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use layer2_core::{
        Event, Page, Reserved, SessionId, SessionListQuery, SessionRecord, StoreError, StoredEvent,
        SubjectId,
    };
    use tower::ServiceExt;

    use super::*;

    #[derive(Default)]
    struct ProbeStore {
        broken: AtomicBool,
        pings: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl SessionStore for ProbeStore {
        async fn ping(&self) -> Result<(), StoreError> {
            self.pings.fetch_add(1, Ordering::SeqCst);
            if self.broken.load(Ordering::SeqCst) {
                Err(StoreError::Unavailable("test store is down".into()))
            } else {
                Ok(())
            }
        }

        async fn get(&self, _id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn list_for(
            &self,
            _owner: &SubjectId,
            _query: &SessionListQuery,
        ) -> Result<Page<SessionRecord>, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn list_all(
            &self,
            _query: &SessionListQuery,
        ) -> Result<Page<SessionRecord>, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn commit_session_idempotent(
            &self,
            _write_id: &str,
            _record: &SessionRecord,
            _events: &[Event],
        ) -> Result<u64, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn create_session_idempotent(
            &self,
            _write_id: &str,
            _record: &SessionRecord,
            _events: &[Event],
        ) -> Result<u64, StoreError> {
            unreachable!("readiness must only ping the store")
        }

        async fn events_since(
            &self,
            _id: &SessionId,
            _after: u64,
        ) -> Result<Vec<StoredEvent>, StoreError> {
            unreachable!("readiness must only ping the store")
        }
    }

    async fn response(app: &Router, path: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("health request builds"),
            )
            .await
            .expect("health router responds")
    }

    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("health response body reads");
        serde_json::from_slice(&bytes).expect("health response is JSON")
    }

    #[tokio::test]
    async fn readiness_is_ok_when_the_store_answers() {
        let store = Arc::new(ProbeStore::default());
        let app = router(Arc::clone(&store) as Arc<dyn SessionStore>);

        let response = response(&app, "/ready").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json(response).await,
            serde_json::json!({ "status": "ready" })
        );
        assert_eq!(store.pings.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_store_outage_fails_readiness_but_not_liveness() {
        let store = Arc::new(ProbeStore::default());
        store.broken.store(true, Ordering::SeqCst);
        let app = router(Arc::clone(&store) as Arc<dyn SessionStore>);

        let readiness = response(&app, "/ready").await;
        assert_eq!(readiness.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            json(readiness).await,
            serde_json::json!({ "status": "not-ready", "reason": "store" })
        );

        let liveness = response(&app, "/livez").await;
        assert_eq!(liveness.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(liveness.into_body(), usize::MAX).await.unwrap(),
            "ok"
        );
        assert_eq!(store.pings.load(Ordering::SeqCst), 1);
    }
}
