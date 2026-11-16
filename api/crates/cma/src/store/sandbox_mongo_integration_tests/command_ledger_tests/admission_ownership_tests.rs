//! Public reservations must fence the local AM and durable owner in the same
//! transaction; atomic creation roots use their own authority instead.
use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};

async fn coordinator(store: Arc<MongoAgentOwnership>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn public_command_reservation_requires_local_and_durable_owner_before_agent_mutation() {
    let (client, name) = database("public_command_owner").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let owners = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let a = coordinator(owners.clone(), "admission-owner").await;
    let old = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = coordinator(owners.clone(), "admission-next").await;
    let one = store
        .clone()
        .enable_agent_ownership("admission-owner".into())
        .await
        .unwrap();
    let two = store
        .clone()
        .enable_agent_ownership("admission-next".into())
        .await
        .unwrap();
    let request = work_tests::live_request(&agent, &parent, "public-owner");
    let before = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let absent = one.reserve_command(request.clone()).await;
    let borrowed = old.run(two.reserve_command(request.clone())).await.unwrap();
    owners.heartbeat("admission-owner", true).await.unwrap();
    owners.release(&old.token).await.unwrap();
    let next = b.acquire(&agent.id).await.unwrap().unwrap();
    assert!(
        old.is_current(),
        "durable fence must fail even before local cancellation"
    );
    let stale = old.run(one.reserve_command(request.clone())).await.unwrap();
    let after_denials = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .await
        .unwrap();
    let denied_count = store.agent_commands.count_documents(doc! {}).await.unwrap();
    let accepted = record(
        next.run(two.reserve_command(request.clone()))
            .await
            .unwrap()
            .unwrap(),
    );
    let replay = record(
        next.run(two.reserve_command(request))
            .await
            .unwrap()
            .unwrap(),
    );
    let final_count = store.agent_commands.count_documents(doc! {}).await.unwrap();
    client.database(&name).drop().await.unwrap();
    assert!(
        matches!(absent, Err(CommandLedgerError::Conflict)),
        "unscoped public reservation must not mutate the Agent"
    );
    assert!(matches!(borrowed, Err(CommandLedgerError::Conflict)));
    assert!(matches!(stale, Err(CommandLedgerError::Conflict)));
    assert_eq!(after_denials, before);
    assert_eq!(denied_count, 0);
    assert_eq!(accepted.receipt, replay.receipt);
    assert_eq!(final_count, 1);
}
