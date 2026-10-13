//! A consented revival cannot supersede a newer owner lifecycle or worker lease.
use super::*;
use layer2_core::sandbox::store::{SandboxEventStore, SandboxStore};
use layer2_core::sandbox::SandboxWorkState;

use crate::store::agent_access_requests::grant::tests::paused_fixture;

async fn granted_fixture() -> (
    mongodb::Client,
    String,
    std::sync::Arc<MongoSandboxStore>,
    AgentAccessRecord,
) {
    let (client, name, store, paused) = paused_fixture().await;
    let record = store
        .grant_access_request_services(paused.lease.as_ref().unwrap())
        .await
        .unwrap();
    (client, name, store, record)
}

async fn parent(store: &MongoSandboxStore, record: &AgentAccessRecord) -> Sandbox {
    store
        .get_sandbox(&record.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn save_parent(store: &MongoSandboxStore, sandbox: &Sandbox) {
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
async fn consented_revival_records_operation_atomically_and_replays_only_its_own_completion() {
    let (client, name, store, record) = granted_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = parent(&store, &record).await;
    let count = store
        .sandbox_events(&before.id, 0, 100)
        .await
        .unwrap()
        .len();
    assert_eq!(
        store.schedule_access_request_revival(lease).await.unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let reviving = parent(&store, &record).await;
    assert_eq!(reviving.status, SandboxStatus::Reviving);
    assert_eq!(reviving.revision, before.revision + 1);
    let saved = store
        .read_access_request(&record.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.phase, AgentAccessPhase::Reviving);
    assert_eq!(
        saved.progress.revive_operation_id.as_deref(),
        Some(
            reviving
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .operation_id
                .as_str()
        )
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened
            .schedule_access_request_revival(lease)
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    assert_eq!(parent(&reopened, &record).await, reviving);
    assert_eq!(
        reopened
            .sandbox_events(&before.id, 0, 100)
            .await
            .unwrap()
            .len(),
        count + 1
    );
    assert_eq!(
        reopened.read_access_request(&record.id).await.unwrap(),
        Some(saved)
    );

    let mut active = reviving;
    active.status = SandboxStatus::Active;
    active.runtime = Some(record.requester.runtime.clone());
    active.lifecycle.operation = None;
    active.lifecycle.home_recovery = None;
    active.lifecycle.paused_notice = None;
    active.work = SandboxWorkState::new(SandboxWorkStep::None, layer2_core::now_rfc3339());
    active.revision += 1;
    save_parent(&store, &active).await;
    assert_eq!(
        store.schedule_access_request_revival(lease).await.unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    // A later independently completed revive has the same status but a new authority.
    active.lifecycle.next_authority += 2;
    active.revision += 1;
    save_parent(&store, &active).await;
    assert_eq!(
        store.schedule_access_request_revival(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(parent(&store, &record).await, active);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn consented_revival_refuses_replaced_lease_conversation_and_newer_pause_without_effects() {
    let (client, name, store, record) = granted_fixture().await;
    let lease = record.lease.as_ref().unwrap();
    let before = parent(&store, &record).await;
    let mut stale = lease.clone();
    stale.token = "newer-worker".into();
    assert_eq!(
        store.schedule_access_request_revival(&stale).await,
        Err(AgentAccessError::Conflict)
    );
    let mut later_pause = before.clone();
    later_pause.lifecycle.next_authority += 2;
    later_pause.revision += 1;
    save_parent(&store, &later_pause).await;
    assert_eq!(
        store.schedule_access_request_revival(lease).await,
        Err(AgentAccessError::Conflict)
    );
    assert_eq!(parent(&store, &record).await, later_pause);
    save_parent(&store, &before).await;
    store
        .agents
        .update_one(
            doc! {"_id":record.requester.agent_id.as_str()},
            doc! {"$set":{"record.conversation.conversation_id":"replaced-native-conversation"}},
        )
        .await
        .unwrap();
    assert!(store.schedule_access_request_revival(lease).await.is_err());
    assert_eq!(parent(&store, &record).await, before);
    assert_eq!(
        store.read_access_request(&record.id).await.unwrap(),
        Some(record)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn consented_revival_snapshot_loses_to_worker_takeover_and_manual_lifecycle() {
    for manual_lifecycle in [false, true] {
        let (client, name, store, record) = granted_fixture().await;
        let lease = record.lease.as_ref().unwrap();
        let before = parent(&store, &record).await;
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        store
            .agent_access_requests
            .find_one(doc! {"_id":&record.id})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        if manual_lifecycle {
            let mut manual = before.clone();
            manual
                .begin_revive(
                    SandboxReviveCause::ExplicitRequest,
                    layer2_core::now_rfc3339(),
                )
                .unwrap();
            save_parent(&store, &manual).await;
        } else {
            store
                .release_access_request(lease, std::time::Duration::ZERO)
                .await
                .unwrap();
            store
                .claim_access_request("newer-worker")
                .await
                .unwrap()
                .unwrap();
        }
        let attempted = apply(&store, lease, Action::Revive, &mut session).await;
        assert!(matches!(attempted, Err(error) if error.transient()));
        let _ = session.abort_transaction().await;
        assert_eq!(
            store.schedule_access_request_revival(lease).await,
            Err(AgentAccessError::Conflict)
        );
        let saved = store
            .read_access_request(&record.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.phase, AgentAccessPhase::Reviving);
        assert!(saved.progress.revive_operation_id.is_none());
        if !manual_lifecycle {
            assert_eq!(parent(&store, &record).await, before);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn consented_revival_retries_transient_writes_and_reconciles_ambiguous_commit_once() {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").expect("failpoint replica set required");
    for ambiguous in [false, true] {
        let (client, name, original, record) = granted_fixture().await;
        let app_name = format!("cma-consented-revive-{}", uuid::Uuid::new_v4().simple());
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app_name.clone());
        let named_client = mongodb::Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&named_client, &name).await.unwrap();
        let before = parent(&original, &record).await;
        let count = original
            .sandbox_events(&before.id, 0, 100)
            .await
            .unwrap()
            .len();
        let (times, data) = if ambiguous {
            (
                5_i32,
                doc! {"failCommands":["commitTransaction"],"appName":&app_name,
                "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous consented revival"}},
            )
        } else {
            (
                1_i32,
                doc! {"failCommands":["update"],"appName":&app_name,"errorCode":112_i32,"errorLabels":["TransientTransactionError"]},
            )
        };
        named_client
            .database("admin")
            .run_command(
                doc! {"configureFailPoint":"failCommand","mode":{"times":times},"data":data},
            )
            .await
            .unwrap();
        let result = store
            .schedule_access_request_revival(record.lease.as_ref().unwrap())
            .await;
        named_client
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
        let saved = store
            .read_access_request(&record.id)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.progress.revive_operation_id.is_some());
        assert_eq!(parent(&store, &record).await.revision, before.revision + 1);
        assert_eq!(
            store
                .sandbox_events(&before.id, 0, 100)
                .await
                .unwrap()
                .len(),
            count + 1
        );
        client.database(&name).drop().await.unwrap();
    }
}
