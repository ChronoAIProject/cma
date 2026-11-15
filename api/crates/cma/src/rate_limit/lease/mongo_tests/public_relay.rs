//! Independent Mongo clients and authenticated HTTP routers charge one shared credit.
use crate::agent_manager::relay::{PublicAccess, Relay};
use crate::{
    rate_limit::{self, RateLimiters},
    store::{request_limits::MongoRequestLimits, MongoSandboxStore},
};
use axum::{http::StatusCode, middleware, routing::get, Router};
use layer2_core::resource_access::{ResourceAccessService, ResourceAccessStore, ResourceTarget};
use layer2_core::sandbox::store::{WorkspaceStore, WorktreeStore};
use layer2_core::{agent_ownership::*, sandbox::*};
use mongodb::Client;
use std::sync::Arc;
use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct NoResources;
#[async_trait::async_trait]
impl SandboxResourceReader for NoResources {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("public preflight cannot invoke runtime resources")
    }
}
struct Serving(tokio::task::JoinHandle<()>);
impl Drop for Serving {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_mongo_routers_authorize_each_hop_and_charge_only_destination() {
    for count in [2, 3] {
        prove(count).await;
    }
}

async fn prove(count: usize) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_public_relay_{}", uuid::Uuid::new_v4().simple());
    let client = Client::with_uri_str(&uri).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, agent) = crate::store::sandbox_maintenance::mongo_integration_tests::fixture(
        &store,
        "sbx_public_relay",
    )
    .await;
    // The maintenance fixture seeds only Sandbox/Agent records. Public policy
    // requires the complete retained ancestry, even for the personal owner.
    store
        .create_workspace(crate::auth::test_resources::workspace(
            agent.workspace_id.as_str(),
            agent.sandbox_id.as_str(),
            agent.owner.as_str(),
        ))
        .await
        .unwrap();
    store
        .create_worktree(crate::auth::test_resources::worktree(
            agent.worktree_id.as_str(),
            agent.workspace_id.as_str(),
            agent.sandbox_id.as_str(),
            agent.id.as_str(),
            agent.owner.as_str(),
        ))
        .await
        .unwrap();
    store
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: agent.id.clone(),
        })
        .await
        .unwrap();
    let effects = Arc::new(AtomicUsize::new(0));
    let mut managers = Vec::new();
    let mut servers = Vec::new();
    let mut updates = Vec::new();
    for index in 0..count {
        // Each app owns its own Mongo client, ownership store and budget instance.
        let client = Client::with_uri_str(&uri).await.unwrap();
        let database = client.database(&name);
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let ownership = Arc::new(
            crate::store::agent_ownership::MongoAgentOwnership::open(&client, &name)
                .await
                .unwrap(),
        );
        let coordinator = OwnershipCoordinator::register(
            ownership,
            Member {
                instance_id: format!("ingress-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        let recorder = DurableAgentUpdateRecorder::new(store.clone());
        let relay = Relay::new(
            coordinator.clone(),
            &recorder,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"public-mongo-relay-test"),
        )
        .unwrap();
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: agent.owner.to_string(),
            },
            None,
        );
        let access = Arc::new(ResourceAccessService::new(store.clone(), auth.clone()));
        let reads = AgentReadService::new(store, Arc::new(NoResources))
            .with_resource_access(access.clone());
        let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
        relay
            .install_public_access(PublicAccess::new(
                auth.clone(),
                reads,
                access,
                Arc::new(adapter),
            ))
            .unwrap();
        let n = |value| NonZeroU32::new(value).unwrap();
        let limits = RateLimiters::from_config(
            crate::config::RateLimitConfig {
                admission_per_minute: n(6000),
                health_per_minute: n(120),
                default_per_minute: n(1),
                create_per_minute: n(10),
                input_per_minute: n(120),
                control_per_minute: n(120),
                control_tier_enabled: true,
                response_input_tier_enabled: true,
                default_lease_enabled: false,
            },
            crate::config::TrustedProxyConfig::sample(),
            Arc::new(MongoRequestLimits::open(&database).await.unwrap()),
        );
        let effects = effects.clone();
        let routes = Router::new()
            .route(
                "/api/v1/agents/{agent}/environment",
                get(move || {
                    let effects = effects.clone();
                    async move {
                        assert!(execution_owner().is_some());
                        effects.fetch_add(1, Ordering::SeqCst);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .layer(middleware::from_fn_with_state(
                limits.clone(),
                rate_limit::enforce,
            ))
            .layer(middleware::from_fn_with_state(
                crate::expensive_reads::ReadBudget::new(false),
                crate::expensive_reads::enforce,
            ));
        let routes = relay
            .authenticated_routes(routes, auth)
            .layer(middleware::from_fn_with_state(limits, rate_limit::admit));
        servers.push(Serving(tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        })));
        updates.push(recorder);
        managers.push((coordinator, address));
    }
    let membership = managers[0].0.store.membership().await.unwrap();
    let owner = candidates(&agent.id, &membership, None).unwrap()[0]
        .instance_id
        .clone();
    let owner_index = managers
        .iter()
        .position(|(manager, _)| manager.instance_id == owner)
        .unwrap();
    let authority = managers[owner_index]
        .0
        .acquire(&agent.id)
        .await
        .unwrap()
        .unwrap();
    let client_http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let source = (owner_index + 1) % count;
    let started = tokio::time::Instant::now();
    let response = client_http
        .get(format!(
            "{}/api/v1/agents/{}/environment",
            managers[source].1, agent.id
        ))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    for (_, address) in &managers {
        let response = client_http
            .get(format!("{address}/api/v1/agents/{}/environment", agent.id))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "rate_limited"
        );
    }
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(authority.is_current());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    for mut server in servers {
        server.0.abort();
        let _ = (&mut server.0).await;
    }
    client.database(&name).drop().await.unwrap();
}
