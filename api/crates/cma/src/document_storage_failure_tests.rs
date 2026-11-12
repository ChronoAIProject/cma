use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};
use serde_json::json;

pub(super) fn client(peer: &Peer) -> Arc<StorageDocuments> {
    StorageDocuments::new(DocumentStorageConfig {
        platform_origin: peer.origin.clone(),
        bucket: "fixture".into(),
        client_id: "fixture-machine".into(),
        client_secret: "local-fixture".to_owned().into(),
    })
    .unwrap()
}

pub(super) fn key() -> String {
    format!(
        "documents/{}/doc_{}/{}.md",
        "a".repeat(64),
        "b".repeat(32),
        digest(b"fixture")
    )
}

pub(super) fn operation(
    client: &Arc<StorageDocuments>,
    write: bool,
) -> tokio::task::JoinHandle<Result<Vec<u8>, DocumentError>> {
    let client = client.clone();
    tokio::spawn(async move {
        if write {
            client
                .put(&key(), b"fixture".to_vec())
                .await
                .map(|()| vec![])
        } else {
            client.get(&key()).await
        }
    })
}

pub(super) async fn authorize(peer: &mut Peer) {
    let request = peer.next().await;
    assert_eq!(request.method, Method::POST);
    assert_eq!(request.uri.path(), "/oauth/token");
    let fields: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect();
    assert_eq!(
        fields,
        std::collections::BTreeMap::from([
            ("grant_type".into(), "client_credentials".into()),
            ("client_id".into(), "fixture-machine".into()),
            ("client_secret".into(), "local-fixture".into()),
            (
                "scope".into(),
                "proxy object:create object:read object:download".into()
            ),
        ])
    );
    request.json(json!({"access_token":"fixture-authority", "expires_in":3600}));
}

#[tokio::test]
async fn pins_storage_proxy_fence_receipt_and_cached_read_contract() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let upload = operation(&client, true);
    authorize(&mut peer).await;
    let request = peer.next().await;
    assert_eq!(request.method, Method::POST);
    assert_eq!(
        request.uri.path(),
        "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects"
    );
    assert_eq!(request.headers["authorization"], "Bearer fixture-authority");
    assert_eq!(
        request.headers["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(request.body, "fixture");
    let query: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(request.uri.query().unwrap().as_bytes())
            .into_owned()
            .collect();
    assert_eq!(
        query,
        std::collections::BTreeMap::from([
            ("key".into(), key()),
            ("contentType".into(), "text/markdown; charset=utf-8".into()),
            ("mutationGeneration".into(), "1".into()),
            ("contentSha256".into(), digest(b"fixture")),
        ])
    );
    request.json(json!({"data":{"key":key(),"contentLength":7,"generation":1,"digest":digest(b"fixture"),"fenceVersion":1}}));
    assert_eq!(finish(upload).await, Ok(vec![]));
    let download = operation(&client, false);
    let request = peer.next().await;
    assert_eq!(request.method, Method::GET);
    assert_eq!(
        request.uri.path(),
        "/api/v1/proxy/s/chrono-storage-service/api/buckets/fixture/objects/download"
    );
    assert_eq!(request.headers["authorization"], "Bearer fixture-authority");
    assert!(request.body.is_empty());
    request.respond(200, "fixture");
    assert_eq!(finish(download).await, Ok(b"fixture".to_vec()));
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn unavailable_storage_is_typed_with_only_retryable_reads_replayed() {
    for write in [false, true] {
        for status in [403, 404, 429, 500, 503] {
            let mut peer = Peer::start().await;
            let client = client(&peer);
            let result = operation(&client, write);
            authorize(&mut peer).await;
            peer.next()
                .await
                .respond(status, "upstream diagnostic must not escape");
            if !write && status == 503 {
                peer.next().await.respond(status, "still unavailable");
            }
            assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
            assert_eq!(peer.count(), if !write && status == 503 { 3 } else { 2 });
        }
    }
}

#[tokio::test]
async fn repeated_unauthorized_storage_is_limited_to_one_refresh_and_replay() {
    for write in [false, true] {
        let mut peer = Peer::start().await;
        let client = client(&peer);
        let result = operation(&client, write);
        for _ in 0..2 {
            authorize(&mut peer).await;
            peer.next().await.respond(401, "refused");
        }
        assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(peer.count(), 4);
        tokio::time::resume();
    }
}

#[tokio::test]
async fn unavailable_token_service_never_contacts_storage() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let result = operation(&client, false);
    peer.next().await.respond(503, "unavailable");
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_eq!(peer.count(), 1);
}

#[tokio::test]
async fn invalid_token_envelopes_never_authorize_object_requests() {
    for envelope in [
        json!({"access_token":"", "expires_in":3600}),
        json!({"access_token":"fixture-authority", "expires_in":119}),
        json!({"access_token":"fixture-authority", "expires_in":86401}),
        json!({"access_token":"fixture-authority"}),
    ] {
        let mut peer = Peer::start().await;
        let client = client(&peer);
        let result = operation(&client, false);
        peer.next().await.json(envelope);
        assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
        assert_eq!(peer.count(), 1);
        assert!(client.token.state.lock().await.cached.is_none());
    }
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let result = operation(&client, false);
    peer.next().await.respond(200, "x".repeat(16 * 1024 + 1));
    assert_eq!(finish(result).await, Err(DocumentError::Corrupt));
    assert_eq!(peer.count(), 1);
    assert!(client.token.state.lock().await.cached.is_none());
}

#[tokio::test]
async fn stalled_token_request_expires_and_releases_refresh_lock_for_recovery() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let result = operation(&client, false);
    let held = peer.next().await;
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    tokio::time::advance(Duration::from_secs(12)).await;
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_deadline(deadline);
    assert!(client.token.state.try_lock().is_ok());
    assert_eq!(peer.count(), 1);
    drop(held);
    // Recovery starts after the bounded negative-cache window.
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::time::resume();
    let recovered = operation(&client, false);
    authorize(&mut peer).await;
    peer.next().await.respond(200, "recovered");
    assert_eq!(finish(recovered).await, Ok(b"recovered".to_vec()));
    assert_eq!(peer.count(), 3);
}

#[tokio::test]
async fn stalled_storage_headers_and_bodies_are_bounded_with_only_one_read_retry() {
    for write in [false, true] {
        for body in [false, true] {
            let mut peer = Peer::start().await;
            let client = client(&peer);
            let result = operation(&client, write);
            authorize(&mut peer).await;
            let request = peer.next().await;
            let held = if body {
                request.stall_body();
                None
            } else {
                Some(request)
            };
            tokio::time::pause();
            let mut deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            tokio::time::advance(Duration::from_secs(12)).await;
            let mut retry_held = None;
            if !write {
                assert!(!result.is_finished());
                assert_deadline(deadline);
                tokio::time::resume();
                let retry = peer.next().await;
                assert_eq!(retry.method, Method::GET);
                if body {
                    retry.stall_body();
                } else {
                    retry_held = Some(retry);
                }
                tokio::time::pause();
                deadline = tokio::time::Instant::now() + Duration::from_secs(12);
                tokio::time::advance(Duration::from_secs(12)).await;
            }
            assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
            assert_deadline(deadline);
            tokio::time::advance(Duration::from_secs(120)).await;
            assert_eq!(peer.count(), if write { 2 } else { 3 });
            drop(held);
            drop(retry_held);
            tokio::time::resume();
        }
    }
}

#[tokio::test]
async fn queued_token_waiters_expire_without_late_provider_calls() {
    let peer = Peer::start().await;
    let client = client(&peer);
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let mut pending = Vec::new();
    for index in 0..32 {
        pending.push(operation(&client, index % 2 == 0));
    }
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(pending.iter().all(|task| !task.is_finished()));
    tokio::time::advance(Duration::from_secs(1)).await;
    for task in pending {
        assert_eq!(finish(task).await, Err(DocumentError::Unavailable));
    }
    drop(guard);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 0);
}

#[tokio::test]
async fn token_wait_and_http_share_one_operation_budget() {
    let mut peer = Peer::start().await;
    let client = client(&peer);
    let guard = client.token.state.lock().await;
    tokio::time::pause();
    let result = operation(&client, false);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(25)).await;
    tokio::time::resume();
    drop(guard);
    let held = peer.next().await;
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(finish(result).await, Err(DocumentError::Unavailable));
    assert_deadline(deadline);
    assert!(client.token.state.try_lock().is_ok());
    assert_eq!(peer.count(), 1);
    drop(held);
}

#[tokio::test]
async fn retirement_archive_reader_requests_no_creation_scope_and_refuses_writes() {
    let mut peer = Peer::start().await;
    let client = StorageDocuments::read_only(DocumentStorageConfig {
        platform_origin: peer.origin.clone(),
        bucket: "fixture".into(),
        client_id: "fixture-machine".into(),
        client_secret: "local-fixture".to_owned().into(),
    })
    .unwrap();
    assert_eq!(
        client.put(&key(), b"fixture".to_vec()).await,
        Err(DocumentError::Invalid)
    );
    let task = operation(&client, false);
    let request = peer.next().await;
    let fields: std::collections::BTreeMap<_, _> = url::form_urlencoded::parse(&request.body)
        .into_owned()
        .collect();
    assert_eq!(
        fields.get("scope").map(String::as_str),
        Some("proxy object:read object:download")
    );
    request.json(json!({"access_token":"fixture-authority","expires_in":3600}));
    let request = peer.next().await;
    assert_eq!(request.method, Method::GET);
    request.respond(200, "fixture");
    assert_eq!(finish(task).await.unwrap(), b"fixture");
    assert_eq!(peer.count(), 2);
}
