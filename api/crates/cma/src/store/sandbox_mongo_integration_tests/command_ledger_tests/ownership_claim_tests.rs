use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use mongodb::bson::Document;
use std::sync::Arc;
use work_tests::{claim_request, live_request};

#[derive(Clone, Copy)]
enum Action {
    Renew,
    Release,
}

impl Action {
    async fn apply(
        self,
        store: &MongoSandboxStore,
        claim: &CommandClaim,
        now: &str,
        expiry: &str,
    ) -> Result<(), CommandLedgerError> {
        match self {
            Self::Renew => {
                store
                    .renew_command_claim(claim, now.into(), expiry.into())
                    .await
            }
            Self::Release => store.release_command_claim(claim, now.into()).await,
        }
    }
}

async fn start(store: Arc<MongoAgentOwnership>, id: &str) -> Arc<OwnershipCoordinator> {
    OwnershipCoordinator::register(
        store,
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{id}:8080"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 256,
        },
    )
    .await
    .unwrap()
}

async fn snapshot(store: &MongoSandboxStore, claim: &CommandClaim) -> Vec<Document> {
    let mut result = Vec::new();
    for (collection, id) in [
        (&store.agent_commands, claim.command_id.as_str()),
        (&store.agent_command_orders, claim.agent_id.as_str()),
        (&store.agents, claim.agent_id.as_str()),
    ] {
        result.push(collection.find_one(doc! {"_id":id}).await.unwrap().unwrap());
    }
    result
}

async fn exercise(action: Action) {
    let (client, name) = database("command_claim_owners").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "lease-owner"))
            .await
            .unwrap(),
    );
    let now = layer2_core::now_rfc3339();
    let request = claim_request(&agent, "original-worker", &now);
    let expiry = request.expires_at.clone();
    let claimed = store.claim_next_command(request).await.unwrap().unwrap();
    let claim = &claimed.work.lease.as_ref().unwrap().claim;
    let owners = Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap());
    let one = store
        .clone()
        .enable_agent_ownership("lease-owner".into())
        .await
        .unwrap();
    let two = store
        .enable_agent_ownership("lease-successor".into())
        .await
        .unwrap();
    let a = start(owners.clone(), "lease-owner").await;
    let old = a.acquire(&agent.id).await.unwrap().unwrap();
    let b = start(owners.clone(), "lease-successor").await;
    let before = snapshot(&one, claim).await;

    assert_eq!(
        action.apply(&one, claim, &now, &expiry).await,
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(snapshot(&one, claim).await, before);
    let unrelated = a
        .acquire(&AgentId("unrelated-agent".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        unrelated
            .run(action.apply(&one, claim, &now, &expiry))
            .await
            .unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(snapshot(&one, claim).await, before);

    // The old local token remains live: only the transaction's durable member
    // and owner checks can refuse this mutation after ownership moves.
    owners.heartbeat("lease-owner", true).await.unwrap();
    owners.release(&old.token).await.unwrap();
    let next = b.acquire(&agent.id).await.unwrap().unwrap();
    assert!(old.is_current());
    assert!(next.token.epoch > old.token.epoch);
    assert_eq!(
        old.run(action.apply(&one, claim, &now, &expiry))
            .await
            .unwrap(),
        Err(CommandLedgerError::Conflict)
    );
    assert_eq!(snapshot(&one, claim).await, before);
    assert_eq!(
        next.run(action.apply(&one, claim, &now, &expiry))
            .await
            .unwrap(),
        Err(CommandLedgerError::Conflict),
        "the previous AM cannot borrow the current owner's token"
    );
    assert_eq!(snapshot(&one, claim).await, before);

    next.run(action.apply(&two, claim, &now, &expiry))
        .await
        .unwrap()
        .unwrap();
    let after = two.get_command(&claim.command_id).await.unwrap().unwrap();
    assert_eq!(after.receipt, admitted.receipt);
    assert_eq!(after.key_sha256, claimed.key_sha256);
    assert_eq!(after.queue_deadline, claimed.queue_deadline);
    assert_eq!(after.revision, claimed.revision + 1);
    assert!(after.work.dispatch.is_none() && after.work.failure.is_none());
    match action {
        Action::Renew => assert_eq!(after.work.lease, claimed.work.lease),
        Action::Release => assert!(after.work.lease.is_none()),
    }
    assert_eq!(two.get_agent(&agent.id).await.unwrap(), Some(agent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn command_claim_renewal_requires_same_local_and_durable_owner() {
    exercise(Action::Renew).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn command_claim_release_requires_same_local_and_durable_owner() {
    exercise(Action::Release).await;
}
