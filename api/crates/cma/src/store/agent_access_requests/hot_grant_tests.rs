#[path = "hot_grant_exact_connection_tests.rs"]
mod exact_connection;

use super::*;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{SandboxStatus, SandboxWorkState, SandboxWorkStep};
use std::sync::Arc;

async fn sandbox(store: &MongoSandboxStore, record: &AgentAccessRecord) -> Sandbox {
    store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap()
}

async fn put(store: &MongoSandboxStore, sandbox: &Sandbox) {
    sandbox.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(sandbox).unwrap()},
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_scope_is_atomic_restart_durable_and_replay_does_not_rotate_again() {
    let (client, name, store, record) = super::super::tests::approved_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = sandbox(&store, &record).await;
    let agent = store
        .agents
        .find_one(doc! {"_id":record.requester.agent_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let count = store
        .sandbox_events(&before.id, 0, 100)
        .await
        .unwrap()
        .len();
    let granted = store
        .grant_live_access_request_services(lease)
        .await
        .unwrap();
    assert_eq!(granted.phase, AgentAccessPhase::Updating);
    let after = sandbox(&store, &record).await;
    assert_eq!(after.status, SandboxStatus::Active);
    assert_eq!(after.runtime, before.runtime);
    assert_eq!(after.lifecycle, before.lifecycle);
    assert_eq!(after.credential.current, before.credential.current);
    assert_eq!(after.credential.reaches, before.credential.reaches);
    assert_eq!(after.profile, before.profile);
    assert_eq!(
        after.credential.key_generation,
        before.credential.key_generation + 1
    );
    assert_eq!(
        after.credential.effective_reaches(),
        vec![ServiceId::new("github")]
    );
    assert_eq!(
        granted.progress.grant_generation_floor,
        Some(after.credential.key_generation)
    );
    assert_eq!(
        granted.progress.hot_update.as_ref().unwrap().runtime,
        before.runtime.clone().unwrap()
    );
    assert!(after.work.is_due(&layer2_core::now_rfc3339()));
    assert_eq!(after.work.step, SandboxWorkStep::WaitForCredentialRotation);
    assert!(granted.progress.pause_operation_id.is_none());
    assert!(granted.progress.revive_operation_id.is_none());
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .grant_live_access_request_services(lease)
            .await
            .unwrap(),
        granted
    );
    assert_eq!(sandbox(&reopened, &record).await, after);
    let after_agent = store
        .agents
        .find_one(doc! {"_id":record.requester.agent_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    // The private concurrency guard advances to fence admission; Agent state,
    // conversation and published revision remain unchanged.
    assert!(
        after_agent.get_i64("resource_share_guard").unwrap()
            > agent.get_i64("resource_share_guard").unwrap()
    );
    let mut after_agent = after_agent;
    let mut agent = agent;
    after_agent.remove("resource_share_guard");
    agent.remove("resource_share_guard");
    assert_eq!(after_agent, agent);
    let events = reopened.sandbox_events(&before.id, 0, 100).await.unwrap();
    assert_eq!(events.len(), count + 1);
    assert!(matches!(
        events.last().unwrap().kind,
        SandboxEventKind::ServiceAccessRevised { .. }
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_scope_refuses_expired_claim_paused_runtime_and_missing_event_authority() {
    let (client, name, store, record) = super::super::tests::approved_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = sandbox(&store, &record).await;
    let mut stale = lease.clone();
    stale.token = "different-worker".into();
    assert_eq!(
        store.grant_live_access_request_services(&stale).await,
        Err(AgentAccessError::Conflict)
    );
    let mut paused = before.clone();
    paused
        .begin_pause(layer2_core::now_rfc3339(), None)
        .unwrap();
    paused.lifecycle.operation = None;
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    put(&store, &paused).await;
    assert_eq!(
        store.grant_live_access_request_services(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(sandbox(&store, &record).await, paused);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record.clone())
    );
    put(&store, &before).await;
    store
        .sandbox_events
        .delete_many(doc! {"sandbox_id":before.id.as_str()})
        .await
        .unwrap();
    assert!(store
        .grant_live_access_request_services(lease)
        .await
        .is_err());
    assert_eq!(sandbox(&store, &record).await, before);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_scope_loses_to_worker_takeover_without_partial_update() {
    let (client, name, store, record) = super::super::tests::approved_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = sandbox(&store, &record).await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_access_requests
        .find_one(doc! {"_id":&record.id})
        .session(&mut session)
        .await
        .unwrap();
    store
        .release_access_request(lease, std::time::Duration::ZERO)
        .await
        .unwrap();
    let winner = store
        .claim_access_request("replacement")
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(apply(&store, lease, &mut session).await, Err(Error::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(sandbox(&store, &record).await, before);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(winner.clone())
    );
    store
        .grant_live_access_request_services(winner.lease.as_ref().unwrap())
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

async fn replace_fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, name, store, mut record) = super::super::tests::approved_fixture().await;
    let mut before = sandbox(&store, &record).await;
    before.credential.reaches = vec![ServiceId::new("baseline"), ServiceId::new("github")];
    put(&store, &before).await;
    record.scope_mode = AgentAccessScopeMode::Replace {
        expected_services: before.credential.effective_reaches(),
    };
    record.expected_scope_revision = Some(0);
    record.proposal.services = vec!["baseline".into()];
    record.body_sha256 = records::digest(&record.proposal).unwrap();
    if let Some(AgentAccessDecision::Approved { services, .. }) = &mut record.decision {
        *services = vec![ServiceId::new("baseline")];
    }
    store
        .agent_access_requests
        .replace_one(doc! {"_id":&record.id}, document(&record).unwrap())
        .await
        .unwrap();
    (client, name, store, record)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_exact_replacement_removes_scope_and_binds_original_runtime() {
    let (client, name, store, record) = replace_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = sandbox(&store, &record).await;
    let granted = store
        .grant_live_access_request_services(lease)
        .await
        .unwrap();
    let after = sandbox(&store, &record).await;
    assert_eq!(
        after.credential.effective_reaches(),
        vec![ServiceId::new("baseline")]
    );
    assert_eq!(after.credential.reaches, before.credential.reaches);
    assert_eq!(after.credential.current, before.credential.current);
    assert_eq!(after.runtime, before.runtime);
    let mut changed = after.clone();
    changed.lifecycle.next_authority += 1;
    put(&store, &changed).await;
    assert_eq!(
        store.grant_live_access_request_services(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(granted)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_stale_expected_scope_cannot_remove_new_scope_or_reintroduce_revoked_scope() {
    let (client, name, store, record) = replace_fixture().await;
    let before = sandbox(&store, &record).await;
    let mut changed = before.clone();
    changed.credential.reaches.push(ServiceId::new("other"));
    put(&store, &changed).await;
    assert_eq!(
        store
            .grant_live_access_request_services(record.lease.as_ref().unwrap())
            .await,
        Err(AgentAccessError::Invalid)
    );
    assert_eq!(sandbox(&store, &record).await, changed);
    put(&store, &before).await;
    let mut changed = before.clone();
    changed
        .revise_live_service_access(
            &before.owner,
            &[ServiceId::new("baseline")],
            layer2_core::now_rfc3339(),
        )
        .unwrap();
    put(&store, &changed).await;
    let mut stale_additive = record.clone();
    stale_additive.scope_mode = AgentAccessScopeMode::Additive;
    if let Some(AgentAccessDecision::Approved { services, .. }) = &mut stale_additive.decision {
        *services = before.credential.effective_reaches();
    }
    store
        .agent_access_requests
        .replace_one(doc! {"_id":&record.id}, document(&stale_additive).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .grant_live_access_request_services(record.lease.as_ref().unwrap())
            .await,
        Err(AgentAccessError::Invalid)
    );
    assert_eq!(sandbox(&store, &record).await, changed);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_hot_existing_baseline_still_requires_a_fresh_delivered_generation() {
    let (client, name, store, record) = super::super::tests::approved_fixture().await;
    let mut before = sandbox(&store, &record).await;
    before.credential.reaches = record.approved_services().unwrap().to_vec();
    put(&store, &before).await;
    let result = store
        .grant_live_access_request_services(record.lease.as_ref().unwrap())
        .await
        .unwrap();
    let after = sandbox(&store, &record).await;
    assert_eq!(
        after.credential.effective_reaches(),
        before.credential.effective_reaches()
    );
    assert!(
        result.progress.grant_generation_floor.unwrap()
            > before.credential.current.as_ref().unwrap().generation
    );
    assert!(after.needs_service_access_credential());
    assert_eq!(after.runtime, before.runtime);
    client.database(&name).drop().await.unwrap();
}
