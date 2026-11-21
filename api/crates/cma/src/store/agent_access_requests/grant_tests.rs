use super::*;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::{Sandbox, SandboxWorkState};
use std::sync::Arc;

pub(in crate::store) async fn paused_fixture() -> (
    mongodb::Client,
    String,
    Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, name, store, record) = super::super::tests::approved_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    store.schedule_access_request_pause(lease).await.unwrap();
    let mut sandbox = store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    sandbox.status = SandboxStatus::Paused;
    sandbox.runtime = None;
    sandbox.lifecycle.operation = None;
    sandbox.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    sandbox.revision += 1;
    sandbox.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":sandbox.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(&sandbox).unwrap()},
        )
        .await
        .unwrap();
    let current = store
        .read_access_request(&record.id)
        .await
        .unwrap()
        .unwrap();
    let record = store
        .advance_access_request(
            lease,
            AgentAccessPhase::Pausing,
            AgentAccessPhase::Granting,
            current.progress,
        )
        .await
        .unwrap();
    (client, name, store, record)
}

async fn sandbox(store: &MongoSandboxStore, record: &AgentAccessRecord) -> Sandbox {
    store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_grant_is_atomic_restart_durable_and_exactly_replayable() {
    let (client, name, store, record) = paused_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = sandbox(&store, &record).await;
    let count = store
        .sandbox_events(&before.id, 0, 100)
        .await
        .unwrap()
        .len();
    let granted = store.grant_access_request_services(lease).await.unwrap();
    assert_eq!(granted.phase, AgentAccessPhase::Reviving);
    let after = sandbox(&store, &record).await;
    assert_eq!(
        after.credential.effective_reaches(),
        vec![ServiceId::new("github")]
    );
    assert_eq!(
        after.credential.key_generation,
        before.credential.key_generation + 1
    );
    assert_eq!(
        granted.progress.grant_generation_floor,
        Some(after.credential.key_generation)
    );
    assert_eq!(after.revision, before.revision + 1);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.grant_access_request_services(lease).await.unwrap(),
        granted
    );
    assert_eq!(sandbox(&reopened, &record).await, after);
    let events = reopened.sandbox_events(&before.id, 0, 100).await.unwrap();
    assert_eq!(events.len(), count + 1);
    assert!(matches!(
        events.last().unwrap().kind,
        SandboxEventKind::ServiceAccessGranted { .. }
    ));
    assert_eq!(
        reopened
            .advance_access_request(
                lease,
                AgentAccessPhase::Granting,
                AgentAccessPhase::Reviving,
                granted.progress
            )
            .await,
        Err(AgentAccessError::Invalid)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_grant_refuses_stale_lease_changed_pause_and_incomplete_event_log() {
    let (client, name, store, record) = paused_fixture().await;
    let before = sandbox(&store, &record).await;
    let lease = record.lease.as_ref().unwrap();
    let mut stale = lease.clone();
    stale.token = "replaced-worker".into();
    assert_eq!(
        store.grant_access_request_services(&stale).await,
        Err(AgentAccessError::Conflict)
    );
    let mut changed = before.clone();
    changed.lifecycle.next_authority += 1;
    changed.revision += 1;
    store
        .sandboxes
        .update_one(
            doc! {"_id":before.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(&changed).unwrap()},
        )
        .await
        .unwrap();
    assert_eq!(
        store.grant_access_request_services(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(sandbox(&store, &record).await, changed);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record.clone())
    );
    store
        .sandboxes
        .update_one(
            doc! {"_id":before.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(&before).unwrap()},
        )
        .await
        .unwrap();
    store
        .sandbox_events
        .delete_many(doc! {"sandbox_id":before.id.as_str()})
        .await
        .unwrap();
    assert!(store.grant_access_request_services(lease).await.is_err());
    assert_eq!(sandbox(&store, &record).await, before);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_grant_loses_to_worker_takeover_without_partial_scope_change() {
    let (client, name, store, record) = paused_fixture().await;
    let before = sandbox(&store, &record).await;
    let lease = record.lease.as_ref().unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    store
        .agent_access_requests
        .find_one(doc! {"_id":&record.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    store
        .release_access_request(lease, std::time::Duration::ZERO)
        .await
        .unwrap();
    let winner = store
        .claim_access_request("replacement-worker")
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(apply(&store,lease,&mut session).await,Err(Error::Driver(error)) if is_transient(&error))
    );
    let _ = session.abort_transaction().await;
    assert_eq!(sandbox(&store, &record).await, before);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(winner.clone())
    );
    store
        .grant_access_request_services(winner.lease.as_ref().unwrap())
        .await
        .unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_grant_existing_scope_keeps_delivered_credential_above_required_floor() {
    let (client, name, store, record) = paused_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let mut before = sandbox(&store, &record).await;
    before.credential.reaches = vec![ServiceId::new("github")];
    before.validate().unwrap();
    store
        .sandboxes
        .update_one(
            doc! {"_id":before.id.as_str()},
            doc! {"$set":sandbox_replacement_fields(&before).unwrap()},
        )
        .await
        .unwrap();
    let count = store
        .sandbox_events(&before.id, 0, 100)
        .await
        .unwrap()
        .len();
    let granted = store.grant_access_request_services(lease).await.unwrap();
    assert_eq!(granted.progress.grant_generation_floor, Some(0));
    assert!(
        before.credential.current.as_ref().unwrap().generation
            > granted.progress.grant_generation_floor.unwrap()
    );
    assert_eq!(
        store.grant_access_request_services(lease).await.unwrap(),
        granted
    );
    assert_eq!(sandbox(&store, &record).await, before);
    assert_eq!(
        store
            .sandbox_events(&before.id, 0, 100)
            .await
            .unwrap()
            .len(),
        count
    );
    client.database(&name).drop().await.unwrap();
}
