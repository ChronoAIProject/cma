use super::failure_tests::{authorize, client, operation};
use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use serde_json::json;

#[tokio::test]
async fn concurrent_callers_share_one_token_refresh() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
    authorize(&mut peer).await;
    for _ in 0..100 {
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        request.respond(200, "fixture");
    }
    for task in tasks {
        assert_eq!(finish(task).await, Ok(b"fixture".to_vec()));
    }
    assert_eq!(peer.count(), 101);
}

#[tokio::test]
async fn cancelled_first_waiter_does_not_cancel_shared_refresh() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let first = operation(&client, false);
    let held = peer.next().await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let tasks: Vec<_> = (0..99).map(|_| operation(&client, false)).collect();
    held.json(json!({"access_token":"fixture-authority", "expires_in":3600}));
    for _ in 0..99 {
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        request.respond(200, "fixture");
    }
    for task in tasks {
        assert_eq!(finish(task).await, Ok(b"fixture".to_vec()));
    }
    assert_eq!(peer.count(), 100);
}

#[tokio::test]
async fn rejected_stale_generation_does_not_evict_newer_token() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let first = operation(&client, false);
    authorize(&mut peer).await;
    let first_request = peer.next().await;
    let second = operation(&client, false);
    let second_request = peer.next().await;
    first_request.respond(401, "rejected");
    // A provider may return identical token bytes for distinct generations.
    authorize(&mut peer).await;
    peer.next().await.respond(200, "first");
    assert_eq!(finish(first).await, Ok(b"first".to_vec()));
    second_request.respond(401, "rejected");
    let replay = peer.next().await;
    assert_eq!(replay.method, Method::GET);
    replay.respond(200, "second");
    assert_eq!(finish(second).await, Ok(b"second".to_vec()));
    assert_eq!(peer.count(), 6);
}

#[tokio::test]
async fn failed_refresh_is_negatively_cached_with_bounded_backoff() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static WINDOW: AtomicU64 = AtomicU64::new(0);
    fn deterministic(delay: Duration) -> Duration {
        WINDOW.store(delay.as_secs(), Ordering::SeqCst);
        delay
    }
    let mut peer = Peer::start().await;
    let mut client = client(&peer);
    Arc::get_mut(&mut client).unwrap().token.jitter = deterministic;
    for (round, seconds) in [2, 4, 8, 16, 30, 30, 30].into_iter().enumerate() {
        let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
        let request = peer.next().await;
        assert_eq!(request.uri.path(), "/oauth/token");
        request.respond(503, "unavailable");
        for task in tasks {
            assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
        }
        assert_eq!(WINDOW.load(Ordering::SeqCst), seconds);
        tokio::time::pause();
        let retry_at = client.token.state.lock().await.retry_at;
        let remaining = retry_at.saturating_duration_since(Instant::now());
        tokio::time::advance(remaining - Duration::from_millis(1)).await;
        let before = Instant::now();
        let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
        for task in tasks {
            assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
        }
        assert_eq!(Instant::now(), before, "cached failures must not wait");
        assert_eq!(peer.count(), round + 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::time::resume();
    }
    let recovered = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "recovered");
    finish(recovered).await.unwrap();
    let rejected = operation(&client, false);
    peer.next().await.respond(401, "rejected");
    peer.next().await.respond(503, "unavailable");
    assert_eq!(finish(rejected).await, Err(DocumentError::Unavailable));
    assert_eq!(WINDOW.load(Ordering::SeqCst), 2, "success resets backoff");
    assert_eq!(peer.count(), 11);
}

#[tokio::test]
async fn token_wait_never_exceeds_operation_budget() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
    let held = peer.next().await;
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + OPERATION_TIMEOUT;
    tokio::time::advance(OPERATION_TIMEOUT).await;
    for task in tasks {
        assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
    }
    assert_deadline(deadline);
    drop(held);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 1);
}

#[tokio::test]
async fn refresh_ahead_serves_current_token_and_refreshes_once_in_background() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let initial = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "fixture");
    finish(initial).await.unwrap();
    let (_, generation) = client.token.current(None).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2881)).await;
    tokio::time::resume();
    let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
    let mut refresh = None;
    let mut reads = 0;
    while refresh.is_none() || reads < 100 {
        let request = peer.next().await;
        if request.uri.path() == "/oauth/token" {
            assert!(refresh.is_none(), "only one refresh may be in flight");
            refresh = Some(request);
        } else {
            assert_eq!(request.headers["authorization"], "Bearer fixture-authority");
            request.respond(200, "fixture");
            reads += 1;
        }
    }
    for task in tasks {
        finish(task).await.unwrap();
    }
    refresh
        .unwrap()
        .json(json!({"access_token":"replacement", "expires_in":3600}));
    assert_eq!(peer.count(), 103);
    let (replacement, newer) = client.token.current(Some(generation)).await.unwrap();
    assert_eq!(replacement.as_str(), "replacement");
    assert_ne!(generation, newer);
    let task = operation(&client, false);
    let request = peer.next().await;
    assert_eq!(request.headers["authorization"], "Bearer replacement");
    request.respond(200, "replacement read");
    finish(task).await.unwrap();
    assert_eq!(peer.count(), 104);
}

#[tokio::test]
async fn hard_expiry_margin_waits_for_the_existing_background_refresh() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let first = operation(&client, false);
    peer.next()
        .await
        .json(json!({"access_token":"current", "expires_in":360}));
    peer.next().await.respond(200, "fixture");
    finish(first).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(299)).await;
    tokio::time::resume();
    let ahead = operation(&client, false);
    let mut refresh = None;
    let mut read = false;
    while refresh.is_none() || !read {
        let request = peer.next().await;
        if request.uri.path() == "/oauth/token" {
            refresh = Some(request);
        } else {
            assert_eq!(request.headers["authorization"], "Bearer current");
            request.respond(200, "fixture");
            read = true;
        }
    }
    finish(ahead).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2)).await;
    let pending = operation(&client, false);
    tokio::task::yield_now().await;
    assert!(!pending.is_finished());
    assert_eq!(peer.count(), 4);
    tokio::time::resume();
    refresh
        .unwrap()
        .json(json!({"access_token":"replacement", "expires_in":360}));
    let request = peer.next().await;
    assert_eq!(request.headers["authorization"], "Bearer replacement");
    request.respond(200, "refreshed");
    assert_eq!(finish(pending).await, Ok(b"refreshed".to_vec()));
    assert_eq!(peer.count(), 5);
}

#[tokio::test]
async fn failed_refresh_ahead_preserves_usable_token_during_backoff() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let initial = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "fixture");
    finish(initial).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(2881)).await;
    tokio::time::resume();
    let ahead = operation(&client, false);
    for _ in 0..2 {
        let request = peer.next().await;
        if request.uri.path() == "/oauth/token" {
            request.respond(503, "unavailable");
        } else {
            request.respond(200, "fixture");
        }
    }
    finish(ahead).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while client.token.state.lock().await.retry_at <= Instant::now() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let tasks: Vec<_> = (0..100).map(|_| operation(&client, false)).collect();
    for _ in 0..100 {
        let request = peer.next().await;
        assert_eq!(request.method, Method::GET);
        assert_eq!(request.headers["authorization"], "Bearer fixture-authority");
        request.respond(200, "fixture");
    }
    for task in tasks {
        finish(task).await.unwrap();
    }
    assert_eq!(peer.count(), 104);
}
