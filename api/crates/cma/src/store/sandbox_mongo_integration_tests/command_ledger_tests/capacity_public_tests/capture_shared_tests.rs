use super::*;
use crate::store::{
    agent_observation::MongoAgentObservation, agent_ownership::MongoAgentOwnership,
};
use layer2_core::{
    agent_ownership::{Member, OwnershipCoordinator, PROTOCOL_VERSION},
    sandbox::agent_observation::{
        observation_scope, AgentObservationAuthority, AgentObservationService,
    },
};

pub(super) async fn observe(
    reads: Arc<AgentReadService>,
    store: &Arc<MongoSandboxStore>,
    agent: &Agent,
    client: &Client,
    database: &str,
) -> (Arc<AgentReadService>, Arc<AgentObservationAuthority>) {
    let native = Arc::new(MongoAgentOwnership::open(client, database).await.unwrap());
    let coordinator = OwnershipCoordinator::register(
        native,
        Member {
            instance_id: "capture-observer".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap();
    let observation = MongoAgentObservation::open(
        Arc::new((**store).clone().with_access_snapshot_session(true)),
        coordinator,
    )
    .await
    .unwrap();
    let custody = observation
        .destination(&agent.id, &agent.created_at)
        .await
        .unwrap()
        .authority;
    let service = AgentObservationService::new(reads.clone(), observation);
    (reads.with_observation(service), custody)
}

pub(super) async fn route<T>(
    custody: Option<&Arc<AgentObservationAuthority>>,
    response: impl std::future::Future<Output = T>,
) -> T {
    match custody {
        Some(custody) => observation_scope(custody.clone(), response).await,
        None => response.await,
    }
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn shared_indexed_capture_advance_preserves_the_original_post_after_opening_frames() {
    tokio::time::timeout(
        Duration::from_secs(25),
        exercise(TurnStopReason::EndTurn, true, true),
    )
    .await
    .unwrap();
}
