use super::*;
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use mongodb::Client;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, SystemTime};

pub(super) async fn assert_held_legacy_replay(
    fixture: &Fixture,
    launch: &Launch,
    request: AgentAdmission,
    initiation: LaunchInitiation,
    agent: &Agent,
) {
    assert!(launch.input_protocol.is_none());
    let launch_rows = fixture
        .seed
        .client
        .database(&fixture.seed.database)
        .collection::<Document>(LAUNCHES);
    let launch_filter = doc! {"_id": launch.id.as_str()};
    let before = launch_rows
        .find_one(launch_filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(before.contains_key("agent_initiation_fence"));
    let parent_filter = doc! {"_id": fixture.parent.id.as_str()};
    let held = fixture
        .resources
        .sandboxes
        .update_one(
            parent_filter.clone(),
            doc! {"$set": {LEDGER_MAINTENANCE_FIELD: null}},
        )
        .await
        .unwrap();
    assert_eq!(held.matched_count, 1);
    let replay = fixture
        .resources
        .admit_agent_with_trigger_initiator(request, initiation, false)
        .await
        .unwrap();
    assert_eq!(&replay.creation().agent, agent);
    assert_eq!(
        launch_rows.find_one(launch_filter).await.unwrap(),
        Some(before)
    );
    fixture
        .resources
        .sandboxes
        .update_one(
            parent_filter,
            doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}},
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn trigger_initiation_expiry_after_read_refuses_fresh_agent_creation() {
    let fixture = Fixture::new(false).await;
    let launch = fixture
        .ready_for_agent(fixture.launch("fresh-expiring-claim").await)
        .await;
    let request = fixture.admission(&launch).await;
    let agent_id = request.agent_id.clone();
    let initiation = launch.initiation.unwrap();
    let reached_write = Arc::new(tokio::sync::Notify::new());
    let observed = reached_write.clone();
    let (resume, released) = mpsc::sync_channel(1);
    let released = Mutex::new(released);
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.command.get_str("update").ok() == Some(LAUNCHES)
            && event.command.get_array("updates").is_ok_and(|updates| {
                updates.iter().any(|update| {
                    update
                        .as_document()
                        .and_then(|update| update.get_document("u").ok())
                        .and_then(|update| update.get_document("$inc").ok())
                        .is_some_and(|increment| increment.contains_key("agent_initiation_fence"))
                })
            })
        {
            observed.notify_one();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
        }
    }));
    let monitored = crate::store::MongoSandboxStore::open(
        &Client::with_options(options).unwrap(),
        &fixture.seed.database,
    )
    .await
    .unwrap();
    let database = fixture.seed.client.database(&fixture.seed.database);
    let launch_rows = database.collection::<Document>(LAUNCHES);
    let launch_filter = doc! {"_id": launch.id.as_str()};
    let expires_at = SystemTime::now() + Duration::from_secs(10);
    let expiry = layer2_core::rfc3339_at(expires_at);
    let updated = launch_rows
        .update_one(
            launch_filter.clone(),
            doc! {"$set": {
                "record.work.lease.expires_at": &expiry, "work_lease_expires_at": &expiry,
            }},
        )
        .await
        .unwrap();
    assert_eq!(updated.matched_count, 1);
    let before = launch_rows
        .find_one(launch_filter.clone())
        .await
        .unwrap()
        .unwrap();
    let inserting = tokio::spawn(async move {
        monitored
            .admit_agent_with_trigger_initiator(request, initiation, false)
            .await
    });
    tokio::time::timeout(Duration::from_secs(8), reached_write.notified())
        .await
        .expect("admission did not validate a live claim and reach the fresh fence");
    tokio::time::sleep(
        expires_at
            .duration_since(SystemTime::now())
            .unwrap_or_default(),
    )
    .await;
    resume.send(()).unwrap();
    assert_eq!(inserting.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(
        launch_rows.find_one(launch_filter).await.unwrap(),
        Some(before)
    );
    assert!(fixture
        .resources
        .get_agent(&agent_id)
        .await
        .unwrap()
        .is_none());
    let provenance = database.collection::<Document>("agent_creation_provenance");
    assert!(provenance
        .find_one(doc! {"_id": agent_id.as_str()})
        .await
        .unwrap()
        .is_none());
    fixture.close().await;
}
