//! Advisory maintenance response after auth/rate admission, never a durable permit.
use super::Session;
use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

#[async_trait::async_trait]
trait AdmissionStatus: Send + Sync {
    async fn active(&self) -> bool;
}
#[async_trait::async_trait]
impl AdmissionStatus for Session {
    async fn active(&self) -> bool {
        self.current().await.is_ok()
    }
}

pub(crate) fn guard(router: axum::Router, session: Option<Arc<Session>>) -> axum::Router {
    match session {
        Some(session) => with_status(router, session),
        None => router,
    }
}
fn with_status(router: axum::Router, status: Arc<dyn AdmissionStatus>) -> axum::Router {
    router.layer(axum::middleware::from_fn_with_state(status, inspect))
}
async fn inspect(
    State(status): State<Arc<dyn AdmissionStatus>>,
    request: Request,
    next: Next,
) -> Response {
    if !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        if request
            .extensions()
            .get::<layer1_public::AuthenticatedSubject>()
            .is_none()
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if !status.active().await {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    }
    // The head may freeze immediately after this read. Domain/workflow and
    // provider writes require their own atomic admission; no HTTP status or
    // handler return settles them. Existing Layer1 idempotency stays untouched.
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        routing::post,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tower::ServiceExt;
    struct Gate {
        active: AtomicBool,
        reads: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl AdmissionStatus for Gate {
        async fn active(&self) -> bool {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.active.load(Ordering::SeqCst)
        }
    }
    fn request(key: &str, authenticated: bool) -> Request {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri("/operation")
            .header("idempotency-key", key)
            .body(Body::from("same-body"))
            .unwrap();
        if authenticated {
            request
                .extensions_mut()
                .insert(layer1_public::AuthenticatedSubject(layer2_core::SubjectId(
                    "owner".into(),
                )));
        }
        request
    }
    fn app(gate: Arc<Gate>) -> axum::Router {
        with_status(
            axum::Router::new().route(
                "/operation",
                post(|request: Request| async {
                    let key = request
                        .headers()
                        .get("idempotency-key")
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned();
                    let body = to_bytes(request.into_body(), 100).await.unwrap();
                    format!("{key}:{}", String::from_utf8(body.to_vec()).unwrap())
                }),
            ),
            gate,
        )
    }
    #[tokio::test]
    async fn unauthenticated_refusal_never_reads_or_mutates_release_authority() {
        let gate = Arc::new(Gate {
            active: AtomicBool::new(true),
            reads: AtomicUsize::new(0),
        });
        for key in ["one", "two", "three"] {
            assert_eq!(
                app(gate.clone())
                    .oneshot(request(key, false))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(gate.reads.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn frozen_status_refuses_before_handler_without_a_durable_obligation() {
        let gate = Arc::new(Gate {
            active: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
        });
        assert_eq!(
            app(gate.clone())
                .oneshot(request("one", true))
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(gate.reads.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn distinct_and_replayed_keys_reach_existing_route_logic_unchanged() {
        let gate = Arc::new(Gate {
            active: AtomicBool::new(true),
            reads: AtomicUsize::new(0),
        });
        for key in ["one", "two", "one"] {
            let response = app(gate.clone()).oneshot(request(key, true)).await.unwrap();
            let body = to_bytes(response.into_body(), 100).await.unwrap();
            assert_eq!(body.as_ref(), format!("{key}:same-body").as_bytes());
        }
        assert_eq!(gate.reads.load(Ordering::SeqCst), 3);
    }
}
