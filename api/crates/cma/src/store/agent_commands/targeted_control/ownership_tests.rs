//! The real targeted/queued transactions must reject a non-owner before writes.
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture as seed_agent};
use crate::store::sandbox_mongo::MongoSandboxStore;
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    record, work_tests::live_request,
};
use futures_util::TryStreamExt;
use layer2_core::agent_ownership::{
    Authority, Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer2_core::sandbox::{agent_io::*, store::*, Agent};
use mongodb::bson::{doc, Document};
use std::sync::Arc;

mod fixture;
use fixture::{queued, targeted};

#[derive(Clone)]
enum Mutation {
    Targeted(Box<TargetedControlReservation>),
    Edit(Box<QueuedInputReservation>),
    Cancel(Box<QueuedCancellationReservation>),
}
#[derive(Debug, PartialEq)]
enum Outcome {
    Targeted(Box<AdmittedTargetedControl>),
    Edit(QueuedInputOutcome),
    Cancel(QueuedCancellationOutcome),
}
impl Mutation {
    async fn run(&self, store: &MongoSandboxStore) -> Result<Outcome, CommandLedgerError> {
        match self {
            Self::Targeted(request) => store
                .reserve_targeted_control(*request.clone())
                .await
                .map(|r| Outcome::Targeted(Box::new(r))),
            Self::Edit(request) => store
                .edit_queued_input(*request.clone())
                .await
                .map(Outcome::Edit),
            Self::Cancel(request) => store
                .cancel_queued_response(*request.clone())
                .await
                .map(Outcome::Cancel),
        }
    }
}
async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let mut result = Vec::new();
    for collection in [
        &store.agents,
        &store.sandboxes,
        &store.agent_commands,
        &store.agent_command_orders,
        &store.command_authorization_audits,
    ] {
        result.push(
            collection
                .find(doc! {})
                .sort(doc! {"_id": 1})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}
async fn authority(store: &MongoSandboxStore, agent: &Agent) -> Arc<Authority> {
    OwnershipCoordinator::register(
        store.agent_ownership.clone().unwrap(),
        Member {
            instance_id: "control-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 16,
        },
    )
    .await
    .unwrap()
    .acquire(&agent.id)
    .await
    .unwrap()
    .unwrap()
}
async fn prove(kind: u8) {
    prove_with_parent(kind, 1).await;
}
async fn prove_with_parent(kind: u8, parent_version: i64) {
    let (client, name, raw) = database().await;
    let (agent, mutation) = if kind == 0 {
        let (agent, request) = targeted(&raw).await;
        (agent, Mutation::Targeted(Box::new(request)))
    } else {
        let (agent, request) = if parent_version == 1 {
            queued(&raw).await
        } else {
            fixture::queued_for_schema(&raw, parent_version).await
        };
        let mutation = if kind == 1 {
            Mutation::Edit(Box::new(QueuedInputReservation {
                cancellation: request,
                text: "edited once".into(),
            }))
        } else {
            Mutation::Cancel(Box::new(request))
        };
        (agent, mutation)
    };
    let store = raw
        .enable_agent_ownership("control-owner".into())
        .await
        .unwrap();
    let remote = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .enable_agent_ownership("other-ingress".into())
        .await
        .unwrap();
    let owner = authority(&store, &agent).await;
    let before = snapshot(&store).await;
    assert_eq!(
        mutation.run(&store).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(
        owner.run(mutation.run(&remote)).await.unwrap().unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, before);
    let original = owner.run(mutation.run(&store)).await.unwrap().unwrap();
    assert_eq!(
        owner.run(mutation.run(&store)).await.unwrap().unwrap(),
        original
    );
    let committed = snapshot(&store).await;
    assert_eq!(
        mutation.run(&store).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, committed);
    if let Mutation::Targeted(request) = &mutation {
        let Outcome::Targeted(expected) = &original else {
            unreachable!()
        };
        assert_eq!(
            remote
                .lookup_targeted_control(*request.clone())
                .await
                .unwrap(),
            Some(*expected.clone())
        );
        assert_eq!(
            snapshot(&store).await,
            committed,
            "receipt lookup must not durably write on another AM"
        );
        assert_eq!(
            store
                .inspect_targeted_control(*request.clone())
                .await
                .unwrap_err(),
            CommandLedgerError::Conflict
        );
        assert_eq!(snapshot(&store).await, committed);
    }
    // The captured scope still has time left, but its durable membership is now
    // draining. This proves the database fence, rather than only a local check.
    store
        .agent_ownership
        .as_ref()
        .unwrap()
        .heartbeat("control-owner", true)
        .await
        .unwrap();
    assert_eq!(
        owner.run(mutation.run(&store)).await.unwrap().unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, committed);
    client.database(&name).drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn targeted_admission_and_replay_require_owner_while_lookup_does_not_write() {
    prove(0).await;
}
#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn queued_edit_and_replay_require_current_durable_owner() {
    prove(1).await;
}
#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn queued_cancel_and_replay_require_current_durable_owner() {
    prove(2).await;
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn queued_edit_accepts_renewable_and_prepared_parents_with_owner_fences() {
    for version in [3, 4] {
        prove_with_parent(1, version).await;
    }
}
#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn queued_cancel_accepts_renewable_and_prepared_parents_with_owner_fences() {
    for version in [3, 4] {
        prove_with_parent(2, version).await;
    }
}
