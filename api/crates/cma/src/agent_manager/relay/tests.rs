use super::*;
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
use tokio::task::JoinHandle;

struct Instance {
    coordinator: Arc<OwnershipCoordinator>,
    relay: Arc<Relay>,
    updates: Arc<DurableAgentUpdateRecorder>,
    address: String,
    server: JoinHandle<()>,
}
impl Drop for Instance {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn cluster(count: usize) -> (Arc<MemoryOwnershipStore>, Vec<Instance>) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let mut instances = Vec::new();
    for index in 0..count {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store.client(),
            Member {
                instance_id: format!("instance-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 256,
            },
        )
        .await
        .unwrap();
        let updates = DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-test-deployment-key"),
        )
        .unwrap();
        updates.install_live_source(relay.live_source());
        let router = relay.routes(
            Router::new()
                .route(
                    "/api/v1/agents/{agent}/{*operation}",
                    axum::routing::any(|headers: HeaderMap, body: axum::body::Bytes| async move {
                        if headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            != Some("Bearer synthetic-authorized")
                        {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        let owner = layer2_core::agent_ownership::execution_owner().unwrap();
                        axum::Json(
                            serde_json::json!({"instance": owner.instance_id, "epoch": owner.epoch, "bytes": body.len(),
                "idempotency": headers.get("idempotency-key").and_then(|value| value.to_str().ok()),
                "request": headers.get("x-request-id").and_then(|value| value.to_str().ok()) }),
                        )
                        .into_response()
                    }),
                )
                .route(
                    "/api/v1/agents/{agent}/files/slow",
                    get(|| async {
                        use futures_util::StreamExt;
                        axum::body::Body::from_stream(
                            futures_util::stream::once(async { Ok::<_, std::io::Error>("first") })
                                .chain(futures_util::stream::pending()),
                        )
                    }),
                )
                .layer(axum::extract::DefaultBodyLimit::max(MAX_BUFFERED_BODY_BYTES)),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        instances.push(Instance {
            coordinator,
            relay,
            updates,
            address,
            server,
        });
    }
    (store, instances)
}

async fn acquire(
    store: &MemoryOwnershipStore,
    instances: &[Instance],
    id: &AgentId,
) -> (usize, Arc<Authority>) {
    let preferred = candidates(id, &store.membership().await.unwrap(), None).unwrap()[0]
        .instance_id
        .clone();
    let index = instances
        .iter()
        .position(|instance| instance.coordinator.instance_id == preferred)
        .unwrap();
    let authority = instances[index]
        .coordinator
        .acquire(id)
        .await
        .unwrap()
        .unwrap();
    (index, authority)
}

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
                .header("x-request-id", "same-request")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body: serde_json::Value = response.json().await.unwrap();
            assert_eq!(body["instance"], instances[owner].coordinator.instance_id);
            assert_eq!(body["epoch"], authority.token.epoch);
            assert_eq!(body["idempotency"], "same-operation");
            assert_eq!(body["request"], "same-request");
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
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
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
                Ok(fragment) => return fragment,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("live fragment delivered across instances")
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
    let sign = |token: &OwnershipToken| {
        peer.relay
            .authentication
            .sign(
                &peer.coordinator.instance_id,
                token,
                &Method::GET,
                &uri,
                &HeaderMap::new(),
                &[],
            )
            .unwrap()
    };
    let proof = sign(&authority.token);
    let accepted = client()
        .get(&url)
        .header(PROOF, &proof)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    drop(accepted);
    assert_eq!(
        client()
            .get(&url)
            .header(PROOF, proof)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        client()
            .get(&url)
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
        client()
            .get(&url)
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
    let mut response = client().get(&url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.chunk().await.unwrap().unwrap(), "first");
    assert_eq!(peer.relay.slots.available_permits(), 0);
    assert_eq!(
        client().get(&url).send().await.unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
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
