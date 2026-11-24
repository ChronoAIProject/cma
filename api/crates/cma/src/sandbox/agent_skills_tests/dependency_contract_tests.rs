use super::*;
use crate::dependency_test_support::{assert_deadline, finish, Peer};

fn client(peer: &Peer) -> OrnnAgentSkillResolver {
    let client = super::super::test_support::scoped_peer(peer, TestCredentials::present());
    Arc::try_unwrap(client).ok().unwrap()
}

fn resolve(
    client: OrnnAgentSkillResolver,
    count: usize,
) -> tokio::task::JoinHandle<Result<Vec<ResolvedAgentSkill>, AgentSkillResolutionError>> {
    tokio::spawn(async move {
        let references = (0..count)
            .map(|i| reference(&format!("skill_{i}"), Some("2.3")))
            .collect::<Vec<_>>();
        client.resolve(&sandbox(), &references).await
    })
}

#[tokio::test]
async fn pins_32_reference_four_inflight_exact_version_and_digest_contract() {
    let mut peer = Peer::start().await;
    let result = resolve(client(&peer), 32);
    for batch in 0..8 {
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(peer.next().await);
        }
        assert_eq!(peer.count(), (batch + 1) * 4);
        // Reverse responses to ensure arrival order cannot change profile order.
        for request in held.into_iter().rev() {
            assert_eq!(request.method, axum::http::Method::GET);
            assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
            assert!(request.body.is_empty());
            assert_eq!(request.uri.query(), Some("version=2.3"));
            let id = request
                .uri
                .path()
                .strip_prefix("/api/v1/proxy/s/ornn-api/api/v1/skills/")
                .unwrap()
                .to_owned();
            request.json(material(&id, "2.3", &"a".repeat(64)));
        }
    }
    let resolved = finish(result).await.unwrap();
    assert_eq!(resolved.len(), 32);
    for (index, item) in resolved.iter().enumerate() {
        assert_eq!(item.id.as_str(), format!("skill_{index}"));
        assert_eq!(item.version, "2.3");
        assert_eq!(item.archive_sha256, "a".repeat(64));
    }
    assert_eq!(peer.count(), 32);
    let refused = finish(resolve(client(&peer), 33)).await;
    assert!(matches!(
        refused,
        Err(AgentSkillResolutionError::Invalid(_))
    ));
    assert_eq!(peer.count(), 32);
}

#[tokio::test]
async fn unavailable_metadata_returns_typed_error_without_retry() {
    for status in [401, 408, 429, 500, 502, 503, 504] {
        let mut peer = Peer::start().await;
        let result = resolve(client(&peer), 1);
        peer.next()
            .await
            .respond(status, "diagnostic must not escape");
        assert!(matches!(
            finish(result).await,
            Err(AgentSkillResolutionError::Unavailable(_))
        ));
        assert_eq!(peer.count(), 1);
    }
}

#[tokio::test]
async fn stalled_metadata_cancels_four_inflight_without_scheduling_the_rest() {
    let mut peer = Peer::start().await;
    let result = resolve(client(&peer), 32);
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(peer.next().await);
    }
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(matches!(
        finish(result).await,
        Err(AgentSkillResolutionError::Unavailable(_))
    ));
    assert_deadline(deadline);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 4);
    drop(held);
}

#[tokio::test]
async fn stalled_metadata_body_is_inside_request_deadline() {
    let mut peer = Peer::start().await;
    let result = resolve(client(&peer), 1);
    peer.next().await.stall_body();
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(matches!(
        finish(result).await,
        Err(AgentSkillResolutionError::Unavailable(_))
    ));
    assert_deadline(deadline);
    assert_eq!(peer.count(), 1);
}

#[tokio::test]
async fn credential_lookup_is_inside_the_30_second_resolution_budget() {
    let peer = Peer::start().await;
    let credentials = TestCredentials::with(CredentialReply::Pending);
    let mut client = client(&peer);
    client.credentials = Some(credentials.clone());
    tokio::time::pause();
    let result = resolve(client, 32);
    tokio::task::yield_now().await;
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 1);
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!result.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        finish(result).await,
        Err(AgentSkillResolutionError::Unavailable(
            "Ornn resolution deadline elapsed".into()
        ))
    );
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 0);
    assert_eq!(credentials.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn successive_lookup_batches_share_one_30_second_resolution_budget() {
    let mut peer = Peer::start().await;
    let result = resolve(client(&peer), 32);
    for seconds in [10, 10, 9] {
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(peer.next().await);
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(seconds)).await;
        tokio::time::resume();
        for request in held {
            let id = request.uri.path().rsplit('/').next().unwrap().to_owned();
            request.json(material(&id, "2.3", &"a".repeat(64)));
        }
    }
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(peer.next().await);
    }
    tokio::time::pause();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        finish(result).await,
        Err(AgentSkillResolutionError::Unavailable(
            "Ornn resolution deadline elapsed".into()
        ))
    );
    assert_deadline(deadline);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(peer.count(), 16);
    drop(held);
}

#[tokio::test]
async fn streamed_metadata_enforces_the_byte_limit_without_content_length() {
    for oversized in [false, true] {
        let mut peer = Peer::start().await;
        let result = resolve(client(&peer), 1);
        let metadata = material("skill_0", "2.3", &"a".repeat(64)).to_string();
        let mut body = metadata.into_bytes();
        body.resize(MAX_RESPONSE_BYTES + usize::from(oversized), b' ');
        // A streaming body leaves Content-Length unknown to exercise the accumulated limit.
        let chunks = body
            .chunks(16 * 1024)
            .map(|chunk| Ok::<_, std::io::Error>(axum::body::Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>();
        peer.next()
            .await
            .respond(200, Body::from_stream(futures_util::stream::iter(chunks)));
        let value = finish(result).await;
        if oversized {
            assert!(matches!(value, Err(AgentSkillResolutionError::Invalid(_))));
        } else {
            assert_eq!(value.unwrap()[0].version, "2.3");
        }
        assert_eq!(peer.count(), 1);
    }
}
