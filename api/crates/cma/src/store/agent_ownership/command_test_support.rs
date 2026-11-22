use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::agent_ownership::{Authority, OwnershipCoordinator, DEFAULT_MAX_OWNED_AGENTS};
use std::sync::Arc;

pub(crate) struct CommandManagers {
    pub store: MongoSandboxStore,
    pub past: Arc<Authority>,
    pub current: Arc<Authority>,
}

pub(crate) async fn handoff(store: &MongoSandboxStore, id: &AgentId) -> CommandManagers {
    let ownership = Arc::new(
        MongoAgentOwnership::open(&store.client, &store.agents.namespace().db)
            .await
            .unwrap(),
    );
    let member = |id: &str| Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    };
    let previous = OwnershipCoordinator::register(ownership.clone(), member("manager-a"))
        .await
        .unwrap();
    let past = previous.acquire(id).await.unwrap().unwrap();
    let successor = OwnershipCoordinator::register(ownership.clone(), member("manager-b"))
        .await
        .unwrap();
    // Leave the old process's local token alive to test the Mongo fence.
    // Only the shared store observes that the old membership is draining.
    ownership.heartbeat("manager-a", true).await.unwrap();
    ownership.release(&past.token).await.unwrap();
    let current = successor.acquire(id).await.unwrap().unwrap();
    assert!(current.token.epoch > past.token.epoch);
    assert!(past.is_current());
    let mut store = store.clone();
    store.agent_ownership = Some(ownership);
    CommandManagers {
        store,
        past,
        current,
    }
}
