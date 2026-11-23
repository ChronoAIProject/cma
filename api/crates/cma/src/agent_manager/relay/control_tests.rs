use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Verifier {
    authentication: authentication::Authentication,
    accepted: AtomicUsize,
}

async fn verify(State(state): State<Arc<Verifier>>, request: Request) -> StatusCode {
    let verified = request
        .headers()
        .get(PROOF)
        .and_then(|value| value.to_str().ok())
        .and_then(|proof| {
            state
                .authentication
                .verify(proof, &Method::POST, request.uri(), request.headers(), &[])
                .ok()
        });
    if verified.is_none_or(|proof| proof.owner != drain_token() || proof.sender != "prestop") {
        return StatusCode::FORBIDDEN;
    }
    assert!(layer1_public::rest::request_id_from_headers(request.headers()).is_some());
    state.accepted.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn drain_client_signs_the_request_id_seen_by_production_middleware() {
    fn key() -> ring::hmac::Key {
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"drain-middleware-fixture")
    }
    let state = Arc::new(Verifier {
        authentication: authentication::Authentication::new(key()),
        accepted: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = crate::with_request_context(
        axum::Router::new()
            .route(DRAIN, axum::routing::post(verify))
            .with_state(state.clone()),
    );
    let _server = Server(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    drain_local(port, key())
        .await
        .expect("signed drain survives the production request middleware");
    assert_eq!(state.accepted.load(Ordering::SeqCst), 1);
    let refused = drain_local(
        port,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"wrong-drain-key"),
    )
    .await
    .unwrap_err();
    assert_eq!(refused, "local Agent Manager rejected drain (HTTP 403)");
    assert_eq!(state.accepted.load(Ordering::SeqCst), 1);
}
