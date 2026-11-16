use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{database, fixture};
use crate::store::sandbox_mongo_integration_tests::command_ledger_tests::{
    record, work_tests::live_request,
};
use futures_util::TryStreamExt;
use layer2_core::agent_ownership::{
    Authority, Member, OwnershipCoordinator, OwnershipStore, OwnershipToken, PROTOCOL_VERSION,
};
use layer2_core::sandbox::{agent_io::*, store::*, Agent, Sandbox};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document};
use std::{sync::Arc, time::Duration};

mod pair;
mod recovery;

async fn owner(store: &MongoSandboxStore, id: &AgentId) -> Arc<Authority> {
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.clone().unwrap(),
        Member {
            instance_id: "admission-owner".into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://127.0.0.1:1".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 32,
        },
    )
    .await
    .unwrap();
    coordinator.acquire(id).await.unwrap().unwrap()
}
fn scoped(token: OwnershipToken) -> Arc<Authority> {
    Authority::new(token, tokio::time::Instant::now() + Duration::from_secs(10))
}
async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<Document>> {
    let mut rows = Vec::new();
    for collection in [
        &store.agents,
        &store.sandboxes,
        &store.agent_commands,
        &store.agent_command_orders,
        &store.command_authorization_audits,
    ] {
        rows.push(
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
    rows
}
fn fake_tokens(current: &OwnershipToken) -> Vec<OwnershipToken> {
    let mut wrong_agent = current.clone();
    wrong_agent.agent_id = AgentId("different-agent".into());
    let mut wrong_instance = current.clone();
    wrong_instance.instance_id = "different-owner".into();
    let mut wrong_epoch = current.clone();
    wrong_epoch.epoch += 1;
    let mut zero_epoch = current.clone();
    zero_epoch.epoch = 0;
    let mut wrong_protocol = current.clone();
    wrong_protocol.protocol += 1;
    vec![
        wrong_agent,
        wrong_instance,
        wrong_epoch,
        zero_epoch,
        wrong_protocol,
    ]
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_URI"]
async fn coordinated_admission_requires_exact_current_local_owner_and_preserves_replay() {
    let (client, name, off) = database().await;
    let (parent, agent) = fixture(&off, "admission-fence").await;
    let store = off
        .enable_agent_ownership("admission-owner".into())
        .await
        .unwrap();
    let remote = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .enable_agent_ownership("nonowner-ingress".into())
        .await
        .unwrap();
    let authority = owner(&store, &agent.id).await;
    let request = live_request(&agent, &parent, "owner-key");
    let before = snapshot(&store).await;
    assert_eq!(
        store.reserve_command(request.clone()).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    for token in fake_tokens(&authority.token) {
        assert_eq!(
            scoped(token)
                .run(store.reserve_command(request.clone()))
                .await
                .unwrap()
                .unwrap_err(),
            CommandLedgerError::Conflict
        );
        assert_eq!(snapshot(&store).await, before);
    }
    assert_eq!(
        authority
            .run(remote.reserve_command(request.clone()))
            .await
            .unwrap()
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, before);
    let accepted = record(
        authority
            .run(store.reserve_command(request.clone()))
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(
        record(
            authority
                .run(store.reserve_command(request.clone()))
                .await
                .unwrap()
                .unwrap()
        ),
        accepted
    );
    let after = snapshot(&store).await;
    assert_eq!(
        store.reserve_command(request.clone()).await.unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, after);
    let ownership = store.agent_ownership.as_ref().unwrap();
    ownership.release(&authority.token).await.unwrap();
    let successor = ownership
        .acquire(&agent.id, "admission-owner")
        .await
        .unwrap()
        .unwrap();
    assert!(successor.token.epoch > authority.token.epoch);
    assert_eq!(
        authority
            .run(store.reserve_command(request.clone()))
            .await
            .unwrap()
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, after);
    let successor = scoped(successor.token);
    assert_eq!(
        record(
            successor
                .run(store.reserve_command(request.clone()))
                .await
                .unwrap()
                .unwrap()
        ),
        accepted
    );
    // Durable draining revokes permission even while the captured task scope
    // still has time remaining. An in-memory token check alone cannot pass this.
    store
        .agent_ownership
        .as_ref()
        .unwrap()
        .heartbeat("admission-owner", true)
        .await
        .unwrap();
    assert_eq!(
        successor
            .run(store.reserve_command(request))
            .await
            .unwrap()
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, after);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_URI"]
async fn noncoordinated_admission_remains_available_without_owner_scope() {
    let (client, name, store) = database().await;
    let (parent, agent) = fixture(&store, "admission-off").await;
    let request = live_request(&agent, &parent, "off-key");
    let first = record(store.reserve_command(request.clone()).await.unwrap());
    assert_eq!(record(store.reserve_command(request).await.unwrap()), first);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica set via CHRONO_TEST_MONGODB_URI"]
async fn admission_owner_fence_and_effect_roll_back_in_the_same_transaction() {
    let (client, name, off) = database().await;
    let (parent, agent) = fixture(&off, "admission-atomic").await;
    let store = off
        .enable_agent_ownership("admission-owner".into())
        .await
        .unwrap();
    let authority = owner(&store, &agent.id).await;
    let owners = client
        .database(&name)
        .collection::<Document>(crate::store::agent_ownership::OWNERS);
    let original_owner = owners
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let before = snapshot(&store).await;
    // Exercise the shared leaf directly so a separate public-root fence
    // cannot hide a missing transactional guard in composed admissions.
    let mut unscoped = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut unscoped)
        .await
        .unwrap();
    let request = live_request(&agent, &parent, "unscoped-leaf");
    let rejected = super::apply(&store, &request, &mut unscoped).await;
    unscoped.abort_transaction().await.unwrap();
    assert!(matches!(
        rejected,
        Err(ApplyError::Domain(CommandLedgerError::Conflict))
    ));
    assert_eq!(snapshot(&store).await, before);
    let mut request = live_request(&agent, &parent, "invalid-revision");
    request.expected_agent_revision += 1;
    assert_eq!(
        authority
            .run(store.reserve_command(request))
            .await
            .unwrap()
            .unwrap_err(),
        CommandLedgerError::Conflict
    );
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        owners
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        original_owner
    );
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let request = live_request(&agent, &parent, "aborted-admission");
    let result = authority
        .run(super::apply(&store, &request, &mut session))
        .await
        .unwrap();
    assert!(result.is_ok());
    session.abort_transaction().await.unwrap();
    assert_eq!(snapshot(&store).await, before);
    assert_eq!(
        owners
            .find_one(doc! {"_id": agent.id.as_str()})
            .await
            .unwrap()
            .unwrap(),
        original_owner
    );
    client.database(&name).drop().await.unwrap();
}
