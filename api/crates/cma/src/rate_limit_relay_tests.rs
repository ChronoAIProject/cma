use super::*;
use axum::{http::StatusCode, routing::get, Router};
use layer2_core::{
    agent_ownership::{
        candidates, test_support::MemoryOwnershipStore, Member, OwnershipCoordinator,
        OwnershipStore, PROTOCOL_VERSION,
    },
    sandbox::{store::InMemorySandboxStore, DurableAgentUpdateRecorder},
};
use tower::ServiceExt;

#[tokio::test]
async fn public_owner_routing_authenticates_and_sheds_before_ownership_io() {
    let ownership = Arc::new(MemoryOwnershipStore::default());
    let coordinator = OwnershipCoordinator::register(
        ownership.clone(),
        Member {
            instance_id: "synthetic-ingress".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap();
    let updates = DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
    let relay = crate::agent_manager::relay::Relay::new(
        coordinator,
        &updates,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-ingress-test"),
    )
    .unwrap();
    // An ownership lookup would fail with 503. Anonymous traffic must be
    // rejected first, even while Mongo is unavailable.
    ownership.partition(true);
    let auth = crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "synthetic".into(),
            client_secret: "synthetic".into(),
            public_url: "http://127.0.0.1:1".into(),
        },
        None,
    );
    let quota = || Quota::per_minute(NonZeroU32::new(1).unwrap());
    let limits = Arc::new(RateLimiters::from_quotas(
        quota(),
        quota(),
        quota(),
        quota(),
        quota(),
        quota(),
        TrustedProxyConfig::sample(),
    ));
    let app = relay
        .authenticated_routes(
            Router::new().route("/api/v1/agents/{agent}/environment", get(anonymous_handler)),
            auth,
        )
        .layer(axum::middleware::from_fn_with_state(limits, admit));
    let first = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/agents/agt-test/environment"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::UNAUTHORIZED);
    for _ in 0..16 {
        let refused = app
            .clone()
            .oneshot(request(Method::GET, "/api/v1/agents/agt-test/environment"))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}

async fn anonymous_handler() -> StatusCode {
    panic!("anonymous request reached the owner handler");
}

#[tokio::test]
async fn a_forwarded_request_charges_the_operation_budget_only_at_the_owner() {
    let ownership = Arc::new(MemoryOwnershipStore::default());
    let admission = || Quota::per_minute(NonZeroU32::new(100).unwrap());
    let operation = || Quota::per_minute(NonZeroU32::new(1).unwrap());
    let limits = Arc::new(RateLimiters::from_quotas(
        admission(),
        admission(),
        operation(),
        operation(),
        operation(),
        operation(),
        TrustedProxyConfig::sample(),
    ));
    let mut instances = Vec::new();
    for index in 0..2 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            ownership.client(),
            Member {
                instance_id: format!("synthetic-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: address,
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        let updates = DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
        let relay = crate::agent_manager::relay::Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-budget-test"),
        )
        .unwrap();
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: "synthetic-caller".into(),
            },
            None,
        );
        let fixture = crate::agent_manager::relay::public_access::fixture::fixture(
            auth.clone(),
            "agt-budget",
            "synthetic-caller",
        )
        .await;
        relay.install_public_access(fixture.policy).unwrap();
        let routes = Router::new()
            .route(
                "/api/v1/agents/{agent}/environment",
                get(|| async { StatusCode::NO_CONTENT }),
            )
            .layer(axum::middleware::from_fn_with_state(
                limits.clone(),
                enforce,
            ));
        let routes = relay
            .authenticated_routes(routes, auth)
            .layer(axum::middleware::from_fn_with_state(limits.clone(), admit));
        let serving = routes.clone();
        let server = tokio::spawn(async move { axum::serve(listener, serving).await.unwrap() });
        instances.push((coordinator, routes, server));
    }
    let id = layer2_core::AgentId("agt-budget".into());
    let preferred = candidates(&id, &ownership.membership().await.unwrap(), None).unwrap()[0]
        .instance_id
        .clone();
    let owner = instances
        .iter()
        .position(|(coordinator, _, _)| coordinator.instance_id == preferred)
        .unwrap();
    let _authority = instances[owner].0.acquire(&id).await.unwrap().unwrap();
    let source = &instances[1 - owner].1;
    let path = "/api/v1/agents/agt-budget/environment";
    // A single shared allowance represents the global budget. If the ingress
    // pod also charges it, the first forwarded request is already rejected.
    let first = source
        .clone()
        .oneshot(request(Method::GET, path))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    let second = source
        .clone()
        .oneshot(request(Method::GET, path))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    for (_, _, server) in instances {
        server.abort();
    }
}
