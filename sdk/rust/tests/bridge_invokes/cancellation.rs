use super::{issue, key, receipt};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    routing::post,
};
use cma_sdk::{Client, bridges::BridgeInvokeReceipt};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Notify};

#[derive(Default)]
struct Observation {
    requests: Mutex<Vec<(String, Vec<u8>)>>,
    committed: Notify,
    release: Notify,
}

async fn handle(
    State(state): State<Arc<Observation>>,
    request: Request<Body>,
) -> (StatusCode, axum::Json<Value>) {
    assert_eq!(
        request.headers()["authorization"],
        "Bearer fixture-dashboard-manager"
    );
    let key = request.headers()["idempotency-key"]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = to_bytes(request.into_body(), 8192).await.unwrap().to_vec();
    let first = {
        let mut observed = state.requests.lock().unwrap();
        observed.push((key, bytes));
        observed.len() == 1
    };
    if first {
        // Simulate a committed command whose HTTP response has not arrived.
        state.committed.notify_one();
        state.release.notified().await;
    }
    (StatusCode::CREATED, axum::Json(receipt(false)))
}

#[tokio::test]
async fn cancellation_stops_observation_and_explicit_retry_preserves_exact_body_and_key() {
    let state = Arc::new(Observation::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::builder(format!("http://{}/proxy", listener.local_addr().unwrap()))
        .unwrap()
        .bearer_token("fixture-dashboard-manager")
        .unwrap()
        .build()
        .unwrap();
    let router = Router::new()
        .route("/proxy/api/v1/bridges/brg-1/invoke-grants", post(handle))
        .with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let first_client = client.clone();
    let caller = tokio::spawn(async move {
        first_client
            .bridges()
            .issue_invoke_grant("brg-1", &issue(), &key())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), state.committed.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    state.release.notify_one();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        state.requests.lock().unwrap().len(),
        1,
        "cancelled future must not launch a retry"
    );
    let retried = client
        .bridges()
        .issue_invoke_grant("brg-1", &issue(), &key())
        .await
        .unwrap();
    let committed: BridgeInvokeReceipt = serde_json::from_value(receipt(false)).unwrap();
    assert_eq!(retried, committed);
    let observed = state.requests.lock().unwrap();
    assert_eq!(observed.len(), 2, "one explicit retry only");
    assert_eq!(
        observed[0], observed[1],
        "retry must retain exact serialized request and key"
    );
    assert_eq!(observed[0].0, key().as_str());
    assert_eq!(
        serde_json::from_slice::<Value>(&observed[0].1).unwrap(),
        serde_json::to_value(issue()).unwrap()
    );
    server.abort();
}
