use super::*;
mod acceptance;
mod cluster_fixture;
mod environment;
mod transport_auth;
use axum::{
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use layer2_core::{
    agent_ownership::{
        candidates, test_support::MemoryOwnershipStore, OwnershipStore, PROTOCOL_VERSION,
    },
    sandbox::{
        store::InMemorySandboxStore, AgentLiveFragment, AgentLiveFragmentKind,
        AgentLiveSubscription,
    },
};

pub(crate) use cluster_fixture::{
    acquire, acquire_for_store, cluster, cluster_with_store, Instance,
};
use cluster_fixture::{cluster_with_capacity, cluster_with_middleware};
use tokio::task::JoinHandle;
use transport_auth::test_auth;

pub(crate) fn public_routes(relay: &Arc<Relay>, router: Router) -> Router {
    relay.routes(router)
}

/// A canonical request ID, which the request context preserves end to end.
const SAME_REQUEST: &str = "00000000-0000-4000-8000-000000000001";

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

#[tokio::test]
async fn two_and_three_instances_route_to_exact_owner_and_preserve_authorization_and_ids() {
    for count in [2, 3] {
        let (store, instances) = cluster(count).await;
        let id = AgentId("agt-routing".into());
        let (owner, authority) = acquire(&store, &instances, &id).await;
        for instance in &instances {
            let url = format!("{}/api/v1/agents/{id}/environment", instance.address);
            let response = client()
                .get(&url)
                .header("authorization", "Bearer synthetic-authorized")
                .header("idempotency-key", "same-operation")
                .header("x-request-id", SAME_REQUEST)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["instance"], instances[owner].coordinator.instance_id);
            assert_eq!(body["epoch"], authority.token.epoch);
            assert_eq!(body["idempotency"], "same-operation");
            assert_eq!(body["request"], SAME_REQUEST);
            assert_eq!(
                client().get(&url).send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let peer = &instances[(owner + 1) % count];
        for (method, path) in [
            (Method::POST, "files"),
            (Method::DELETE, "files"),
            (Method::POST, "attachments"),
            (Method::GET, "browser"),
            (Method::PUT, "browser"),
            (Method::DELETE, "browser"),
            (Method::GET, "browser/control"),
            (Method::POST, "browser/control"),
            (Method::GET, "models"),
            (Method::POST, "skills"),
            (Method::POST, "recover-worktree"),
        ] {
            // Existing file/attachment contracts permit bodies above the usual
            // JSON limit. The relay must preserve those bounds and exact bytes.
            let size = if method == Method::POST && matches!(path, "files" | "attachments") {
                3 * 1024 * 1024
            } else {
                0
            };
            let response = client()
                .request(
                    method,
                    format!("{}/api/v1/agents/{id}/{path}", peer.address),
                )
                .header("authorization", "Bearer synthetic-authorized")
                .body(vec![b'x'; size])
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["instance"], instances[owner].coordinator.instance_id);
            assert_eq!(body["bytes"], size);
        }
        instances[owner].coordinator.fail().await;
        let other = (owner + 1) % count;
        assert_eq!(
            client()
                .get(format!(
                    "{}/api/v1/agents/{id}/environment",
                    instances[other].address
                ))
                .header("authorization", "Bearer synthetic-authorized")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

async fn owner_routed(
    instance: &Instance,
    method: Method,
    id: &AgentId,
    path: &str,
) -> (StatusCode, serde_json::Value) {
    let response = client()
        .request(
            method,
            format!("{}/api/v1/agents/{id}/{path}", instance.address),
        )
        .header("authorization", "Bearer synthetic-authorized")
        .send()
        .await
        .unwrap();
    (response.status(), response.json().await.unwrap())
}

#[tokio::test]
async fn file_requests_without_a_live_owner_are_served_locally_and_other_owner_routes_refuse() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-without-owner".into());
    let files = [
        (Method::GET, "files/content"),
        (Method::GET, "files/tree"),
        (Method::POST, "files"),
        (Method::DELETE, "files"),
    ];
    let others = [
        (Method::GET, "environment"),
        (Method::GET, "models"),
        (Method::POST, "attachments"),
        (Method::POST, "recover-worktree"),
    ];
    // A stopped Agent has no live owner at all.
    for instance in &instances {
        for (method, path) in files.clone() {
            let (status, body) = owner_routed(instance, method, &id, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(body["instance"], serde_json::Value::Null, "{path}");
        }
        for (method, path) in others.clone() {
            let (status, body) = owner_routed(instance, method, &id, path).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
            assert_eq!(body["code"], "agent_owner_unavailable", "{path}");
        }
    }
    // A live owner still serves its Agent's files.
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let peer = &instances[(owner + 1) % instances.len()];
    let (status, body) = owner_routed(peer, Method::GET, &id, "files/content").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["instance"], instances[owner].coordinator.instance_id);
    assert_eq!(body["epoch"], authority.token.epoch);
    // A draining owner cannot accept new work; files fall back to the peer.
    instances[owner].coordinator.begin_drain().await;
    for (method, path) in files {
        let (status, body) = owner_routed(peer, method, &id, path).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body["instance"], serde_json::Value::Null, "{path}");
    }
    for (method, path) in others {
        let (status, body) = owner_routed(peer, method, &id, path).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(body["code"], "agent_owner_unavailable", "{path}");
    }
}

fn fragment(id: &AgentId, text: &str) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: id.clone(),
        runtime_generation: 1,
        turn_sequence: 1,
        after_seq: 1,
        message_id: "synthetic-message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: text.into(),
    }
}

async fn subscribed(instance: &Instance, id: &AgentId, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while instance.updates.live_subscriber_count(id) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every peer established its owner subscription");
}

async fn next(lane: &mut AgentLiveSubscription) -> AgentLiveFragment {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match lane.recv().await {
                Ok(fragment) => return (*fragment).clone(),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("live fragment delivered across instances")
}

#[tokio::test]
async fn peer_live_stream_survives_its_opening_deadline_and_stops_on_owner_release() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-peer-live-deadline".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let peer = instances[1 - owner].relay.clone();
    let mut destination = peer.destination(&id).await.unwrap();
    // Model a connection opened using a deadline observed before the latest
    // renewal. The exact owner and member rows both outlive this snapshot.
    let opening_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    destination.authority = Authority::new(destination.token.clone(), opening_deadline);
    let (publisher, mut subscription) = AgentLiveSubscription::channel();
    let (ready, admitted) = tokio::sync::oneshot::channel();
    let following = tokio::spawn(async move {
        peer.follow_remote(destination, &publisher, &mut Some(ready))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), admitted)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let first = fragment(&id, "before the opening deadline");
    authority
        .run(async {
            instances[owner].updates.publish_live_fixture(first.clone());
        })
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(2), subscription.recv())
            .await
            .unwrap()
            .unwrap(),
        first
    );
    tokio::time::sleep_until(opening_deadline + Duration::from_millis(50)).await;
    let second = fragment(&id, "after the opening deadline");
    authority
        .run(async {
            instances[owner]
                .updates
                .publish_live_fixture(second.clone());
        })
        .await
        .unwrap();
    assert_eq!(
        *tokio::time::timeout(Duration::from_secs(2), subscription.recv())
            .await
            .expect("the same remote stream stayed open past its opening deadline")
            .unwrap(),
        second
    );

    store.release(&authority.token).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), following)
            .await
            .unwrap()
            .unwrap(),
        Err(())
    );
}

#[tokio::test]
async fn two_and_three_instances_fan_out_and_follow_drained_owner_without_changing_durable_projection(
) {
    for count in [2, 3] {
        let (store, instances) = cluster(count).await;
        let id = AgentId("agt-live".into());
        let (owner, authority) = acquire(&store, &instances, &id).await;
        let mut lanes: Vec<_> = instances
            .iter()
            .map(|instance| instance.relay.subscribe(&id).unwrap())
            .collect();
        subscribed(&instances[owner], &id, count).await;
        let first = fragment(&id, "first fragment");
        authority
            .run(async {
                instances[owner].updates.publish_live_fixture(first.clone());
            })
            .await
            .unwrap();
        for lane in &mut lanes {
            assert_eq!(next(lane).await, first);
        }
        instances[owner].coordinator.begin_drain().await;
        instances[owner]
            .coordinator
            .handoff(&authority, async { Ok(()) })
            .await
            .unwrap();
        let (successor, next_authority) = acquire(&store, &instances, &id).await;
        assert_ne!(owner, successor);
        assert!(next_authority.token.epoch > authority.token.epoch);
        subscribed(&instances[successor], &id, count - 1).await;
        assert!(authority
            .run(async {
                instances[owner]
                    .updates
                    .publish_live_fixture(fragment(&id, "stale"));
            })
            .await
            .is_err());
        let second = fragment(&id, "successor fragment");
        next_authority
            .run(async {
                instances[successor]
                    .updates
                    .publish_live_fixture(second.clone());
            })
            .await
            .unwrap();
        for (index, lane) in lanes.iter_mut().enumerate() {
            if index != owner {
                assert_eq!(next(lane).await, second);
            }
        }
    }
}

#[tokio::test]
async fn peer_auth_rejects_unsigned_tampered_replayed_and_stale_epoch_requests() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-auth".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let peer = &instances[(owner + 1) % 2];
    let uri: Uri = format!("{PREFIX}/live/{id}").parse().unwrap();
    let url = format!("{}{}", instances[owner].address, uri);
    assert_eq!(
        client().get(&url).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let mut signed = HeaderMap::new();
    signed.insert("x-request-id", SAME_REQUEST.parse().unwrap());
    let sign_over = |token: &OwnershipToken, headers: &HeaderMap| {
        peer.relay
            .authentication
            .sign(
                &peer.coordinator.instance_id,
                token,
                &Method::GET,
                &uri,
                headers,
                &[],
            )
            .unwrap()
    };
    // The receiver's request context assigns a request ID to a request that
    // arrives without one; a proof that did not sign it is refused (#3261).
    assert_eq!(
        client()
            .get(&url)
            .header(PROOF, sign_over(&authority.token, &HeaderMap::new()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let sign = |token: &OwnershipToken| sign_over(token, &signed);
    let get = || client().get(&url).header("x-request-id", SAME_REQUEST);
    let proof = sign(&authority.token);
    let accepted = get().header(PROOF, &proof).send().await.unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    drop(accepted);
    assert_eq!(
        get().header(PROOF, proof).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get()
            .header(PROOF, sign(&authority.token))
            .header("authorization", "changed")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut old = authority.token.clone();
    old.epoch += 1;
    assert_eq!(
        get()
            .header(PROOF, sign(&old))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn public_lane_bounds_connections_and_revokes_already_buffered_local_frames() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-buffer".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let mut local = instances[owner].updates.subscribe_local(&id);
    authority
        .run(async {
            instances[owner]
                .updates
                .publish_live_fixture(fragment(&id, "buffered"));
        })
        .await
        .unwrap();
    authority.revoke();
    assert!(matches!(
        local.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(1))
    ));
    let subscriptions: Vec<_> = (0..MAX_STREAMS)
        .map(|_| instances[owner].relay.subscribe(&id).unwrap())
        .collect();
    assert!(instances[owner].relay.subscribe(&id).is_none());
    drop(subscriptions);
}

#[path = "mongo_tests.rs"]
mod mongo;

#[path = "head_tests.rs"]
mod heads;

#[tokio::test]
async fn slow_owner_response_keeps_relay_capacity_until_reader_disconnects() {
    let (store, instances) = cluster(2).await;
    let id = AgentId("agt-slow-response".into());
    let (owner, _) = acquire(&store, &instances, &id).await;
    let peer = &instances[1 - owner];
    let reserved = peer
        .relay
        .slots
        .clone()
        .acquire_many_owned((MAX_STREAMS - 1) as u32)
        .await
        .unwrap();
    let url = format!("{}/api/v1/agents/{id}/files/slow", peer.address);
    let mut response = client()
        .get(&url)
        .header("authorization", "Bearer synthetic-authorized")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.chunk().await.unwrap().unwrap(), "first");
    assert_eq!(peer.relay.slots.available_permits(), 0);
    assert_eq!(
        client()
            .get(&url)
            .header("authorization", "Bearer synthetic-authorized")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    drop(response);
    tokio::time::timeout(Duration::from_secs(2), async {
        while peer.relay.slots.available_permits() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(reserved);
}

#[path = "capacity_tests.rs"]
mod capacity_tests;
