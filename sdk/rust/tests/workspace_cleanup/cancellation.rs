use super::fixture::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::Request,
    routing::post,
};
use cma_sdk::{Client, bridges::BridgeWorkspaceCleanupStatus};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Notify};

#[derive(Default)]
struct Observation {
    calls: Mutex<Vec<(String, Vec<u8>)>>,
    committed: Notify,
    release: Notify,
}
async fn handle(
    State(state): State<Arc<Observation>>,
    request: Request<Body>,
) -> (axum::http::StatusCode, axum::Json<Value>) {
    let key = request.headers()["idempotency-key"]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(request.into_body(), 8192).await.unwrap().to_vec();
    let first = {
        let mut calls = state.calls.lock().unwrap();
        calls.push((key, bytes));
        calls.len() == 1
    };
    if first {
        state.committed.notify_one();
        state.release.notified().await;
    }
    (axum::http::StatusCode::ACCEPTED, axum::Json(status()))
}
#[tokio::test]
async fn cancelled_observation_and_explicit_retry_keep_exact_cleanup_request_key_and_receipt() {
    let state = Arc::new(Observation::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder(format!("http://{}/proxy", listener.local_addr().unwrap()))
        .unwrap()
        .bearer_token("fixture-manager")
        .unwrap()
        .build()
        .unwrap();
    let router = Router::new()
        .route(&format!("/proxy{}", path()), post(handle))
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let first_client = client.clone();
    let caller = tokio::spawn(async move {
        first_client
            .bridges()
            .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), state.committed.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    state.release.notify_one();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    let recovered = client
        .bridges()
        .request_workspace_cleanup(BRIDGE, &allocation(), &request(), &key())
        .await
        .unwrap();
    let original: BridgeWorkspaceCleanupStatus = serde_json::from_value(status()).unwrap();
    assert_eq!(recovered, original);
    let observed = state.calls.lock().unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0], observed[1]);
    assert_eq!(
        serde_json::from_slice::<Value>(&observed[0].1).unwrap(),
        serde_json::to_value(request()).unwrap()
    );
    server.abort();
}
