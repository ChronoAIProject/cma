use super::failure_tests::{authorize, client, key, operation};
use super::*;
use crate::dependency_test_support::{finish, Peer};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn get_retries_transport_failure_once_within_budget() {
    for status in [502, 503, 504] {
        let mut peer = Peer::start().await;
        let client = client(&peer);
        let result = operation(&client, false);
        authorize(&mut peer).await;
        peer.next().await.respond(status, "temporary failure");
        let retry = peer.next().await;
        assert_eq!(retry.method, Method::GET);
        retry.respond(200, "recovered");
        assert_eq!(finish(result).await, Ok(b"recovered".to_vec()));
        assert_eq!(peer.count(), 3);
    }
}

#[tokio::test]
async fn get_transport_retry_is_capped_even_when_retry_fails() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let result = operation(&client, false);
    authorize(&mut peer).await;
    for _ in 0..2 {
        peer.next().await.respond(503, "unavailable");
    }
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn unauthorized_replays_at_most_once_across_transport_retry() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let result = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(401, "rejected");
    authorize(&mut peer).await;
    peer.next().await.respond(503, "unavailable");
    peer.next().await.respond(401, "still rejected");
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 5);
}

#[tokio::test]
async fn get_does_not_retry_when_remaining_operation_budget_is_insufficient() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let result = operation(&client, false);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(28)).await;
    tokio::time::resume();
    drop(guard);
    authorize(&mut peer).await;
    peer.next().await.respond(503, "unavailable");
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 2);
}

#[tokio::test]
async fn expired_operation_never_issues_an_object_request_with_a_ready_token() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let initial = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "primed");
    finish(initial).await.unwrap();
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let deadline = Instant::now() + OPERATION_TIMEOUT;
    let pending_client = client.clone();
    let mut pending =
        tokio::spawn(async move { pending_client.get_object(&key(), deadline).await });
    tokio::task::yield_now().await;
    tokio::time::advance(OPERATION_TIMEOUT).await;
    drop(guard);
    tokio::time::resume();
    tokio::select! {
        result = &mut pending => assert_eq!(result.unwrap(), Err(DocumentError::Unavailable)),
        request = peer.next() => {
            request.respond(200, "late request");
            assert_eq!(finish(pending).await, Err(DocumentError::Unavailable));
        }
    }
    assert_eq!(peer.count(), 2);
}

#[tokio::test]
async fn token_wait_retry_and_body_read_share_the_operation_deadline() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let deadline = Instant::now() + OPERATION_TIMEOUT;
    let result = operation(&client, false);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(19)).await;
    drop(guard);
    tokio::time::resume();
    authorize(&mut peer).await;
    peer.next().await.respond(503, "unavailable");
    peer.next().await.stall_body();
    tokio::time::pause();
    tokio::time::advance(deadline.saturating_duration_since(Instant::now())).await;
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    crate::dependency_test_support::assert_deadline(deadline);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn put_never_retries_transport_failure_or_5xx() {
    for status in [500, 502, 503, 504] {
        let mut peer = Peer::start().await;
        let client = client(&peer);
        let result = operation(&client, true);
        authorize(&mut peer).await;
        peer.next().await.respond(status, "unavailable");
        assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
        assert_eq!(peer.count(), 2);
    }
    connection_failure(true).await;
}

#[tokio::test]
async fn get_retries_connection_failure_once_within_budget() {
    connection_failure(false).await;
}

async fn connection_failure(write: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).await.unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|value| value.parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let body = if bytes.starts_with(b"POST /oauth/token ") {
                r#"{"access_token":"fixture-authority","expires_in":3600}"#
            } else if count.fetch_add(1, Ordering::SeqCst) == 0 {
                // Closing before response headers forces a connection failure.
                continue;
            } else {
                "recovered"
            };
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let client = StorageDocuments::new(DocumentStorageConfig {
        resilience_enabled: true,
        platform_origin: origin,
        bucket: "fixture".into(),
        client_id: "fixture-machine".into(),
        client_secret: "local-fixture".to_owned().into(),
    })
    .unwrap();
    let result = finish(operation(&client, write)).await;
    if write {
        assert_eq!(result, Err(DocumentError::Unavailable));
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    } else {
        assert_eq!(result, Ok(b"recovered".to_vec()));
        assert_eq!(seen.load(Ordering::SeqCst), 2);
    }
    server.abort();
}
