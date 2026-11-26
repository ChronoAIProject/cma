use super::*;

pub(crate) struct Instance {
    pub(crate) coordinator: Arc<OwnershipCoordinator>,
    pub(crate) relay: Arc<Relay>,
    pub(super) updates: Arc<DurableAgentUpdateRecorder>,
    pub(super) address: String,
    pub(super) server: JoinHandle<()>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub(crate) async fn cluster(count: usize) -> (Arc<MemoryOwnershipStore>, Vec<Instance>) {
    cluster_with_capacity(count, Default::default()).await
}
pub(super) async fn cluster_with_capacity(
    count: usize,
    capacity: crate::config::RelayCapacityConfig,
) -> (Arc<MemoryOwnershipStore>, Vec<Instance>) {
    cluster_with_middleware(count, capacity, |router| router).await
}

pub(super) async fn cluster_with_middleware(
    count: usize,
    capacity: crate::config::RelayCapacityConfig,
    wrap: impl Fn(Router) -> Router,
) -> (Arc<MemoryOwnershipStore>, Vec<Instance>) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let instances = spawn_cluster(count, capacity, wrap, || store.client()).await;
    (store, instances)
}

pub(crate) async fn cluster_with_store(
    count: usize,
    store: Arc<dyn OwnershipStore>,
    wrap: impl Fn(Router) -> Router,
) -> Vec<Instance> {
    spawn_cluster(count, Default::default(), wrap, || store.clone()).await
}

async fn spawn_cluster(
    count: usize,
    capacity: crate::config::RelayCapacityConfig,
    wrap: impl Fn(Router) -> Router,
    store: impl Fn() -> Arc<dyn OwnershipStore>,
) -> Vec<Instance> {
    let mut instances = Vec::new();
    for index in 0..count {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store(),
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
        let relay = Relay::with_capacity(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-test-deployment-key"),
            capacity,
        )
        .unwrap();
        assert!(relay
            .public_access
            .set(Arc::new(transport_auth::TransportPolicy))
            .is_ok());
        updates.install_live_source(relay.live_source());
        let router = relay.routes(
            Router::new()
                .route("/api/v1/agents/{agent}", get(environment::ready_fixture_read))
                .route(
                    "/api/v1/agents/{agent}/transcript",
                    get(environment::ready_fixture_read),
                )
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
                        // File requests may run locally without an owner.
                        let owner = layer2_core::agent_ownership::execution_owner();
                        axum::Json(
                            serde_json::json!({"instance": owner.as_ref().map(|owner| owner.instance_id.clone()),
                "epoch": owner.as_ref().map(|owner| owner.epoch), "bytes": body.len(),
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
        ).layer(axum::middleware::from_fn(test_auth));
        // Production serves the relay behind the request context, which
        // normalizes or assigns the signed request ID before verification.
        let router = crate::with_request_context(wrap(router));
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
    instances
}

pub(crate) async fn acquire(
    store: &MemoryOwnershipStore,
    instances: &[Instance],
    id: &AgentId,
) -> (usize, Arc<Authority>) {
    acquire_for_store(store, instances, id).await
}

pub(crate) async fn acquire_for_store(
    store: &dyn OwnershipStore,
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
