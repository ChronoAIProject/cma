use super::failure_tests::{authorize, client_with_policy, key, operation};
use super::*;
use crate::dependency_test_support::{finish, Peer};
use serde_json::json;

#[tokio::test]
async fn disabled_policy_keeps_receipt_rejection_without_new_telemetry() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let mut peer = Peer::start().await;
    let client = client_with_policy(&peer, false);
    for generation in [1, 2] {
        let task = operation(&client, true);
        if generation == 1 {
            authorize(&mut peer).await;
        }
        peer.next().await.json(json!({"data": {
            "key":key(), "contentLength":7, "generation":generation,
            "digest":digest(b"fixture"), "fenceVersion":1
        }}));
        assert_eq!(
            finish(task).await,
            if generation == 1 {
                Ok(vec![])
            } else {
                Err(DocumentError::Corrupt)
            }
        );
    }
    assert_eq!(peer.count(), 3);
    assert!(!handle
        .render()
        .contains("cma_document_storage_receipt_mismatch_total"));
}

#[tokio::test]
async fn disabled_policy_keeps_one_get_attempt_and_no_negative_cache() {
    let mut peer = Peer::start().await;
    let client = client_with_policy(&peer, false);
    for _ in 0..2 {
        let task = operation(&client, false);
        let request = peer.next().await;
        assert_eq!(request.uri.path(), "/oauth/token");
        request.respond(503, "unavailable");
        assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
    }
    let task = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(503, "unavailable");
    assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 4);
    let task = operation(&client, false);
    peer.next().await.respond(200, "recovered");
    assert_eq!(finish(task).await, Ok(b"recovered".to_vec()));
    assert_eq!(peer.count(), 5);
}

#[tokio::test]
async fn disabled_policy_preserves_cached_token_without_refresh_ahead() {
    let mut peer = Peer::start().await;
    let client = client_with_policy(&peer, false);
    let task = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "initial");
    finish(task).await.unwrap();
    client.legacy_token.lock().await.as_mut().unwrap().1 =
        std::time::Instant::now() + Duration::from_secs(120);
    let task = operation(&client, false);
    let request = peer.next().await;
    assert_eq!(request.method, Method::GET);
    request.respond(200, "cached");
    assert_eq!(finish(task).await, Ok(b"cached".to_vec()));
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn disabled_policy_retains_only_one_unauthorized_replay() {
    let mut peer = Peer::start().await;
    let client = client_with_policy(&peer, false);
    let task = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(401, "rejected");
    authorize(&mut peer).await;
    peer.next().await.respond(401, "rejected again");
    assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 4);
}

#[tokio::test]
async fn disabled_policy_cancels_the_serial_refresh_with_its_waiter() {
    let mut peer = Peer::start().await;
    let client = client_with_policy(&peer, false);
    let first = operation(&client, false);
    let held = peer.next().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let second = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "recovered");
    assert_eq!(finish(second).await, Ok(b"recovered".to_vec()));
    assert_eq!(peer.count(), 3);
    drop(held);
}
