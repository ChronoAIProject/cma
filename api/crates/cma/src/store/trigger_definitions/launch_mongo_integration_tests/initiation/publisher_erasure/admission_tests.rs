//! A cached accepted Launch cannot create a late Agent after publisher erasure.
use super::*;
use crate::store::account_erasure::{erase_records, Cursor, Target};
use layer2_core::account_deletion::ErasureInventory;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Mutex,
};

pub(super) async fn erase_publisher_records(f: &Fixture) {
    let db = f.seed.client.database(&f.seed.database);
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    for _ in 0..1000 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &f.seed.owner,
                inventory: &inventory,
            }],
            &mut cursor,
            "2026-11-14T00:00:00Z",
        )
        .await
        .unwrap();
        if let Some(delta) = batch.discovered.get(f.seed.owner.as_str()) {
            macro_rules! merge {
                ($($field:ident),+) => { $(for id in &delta.$field {
                    if !inventory.$field.contains(id) { inventory.$field.push(id.clone()); }
                })+ };
            }
            merge!(
                sandboxes, workspaces, worktrees, agents, launches, bindings, triggers, documents
            );
        }
        assert!(batch.pending_key_ids.is_empty());
        if !batch.has_more {
            assert_eq!(
                f.seed
                    .profiles
                    .agent_profile(&f.seed.target.agent_profile.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .owner
                    .as_str(),
                "deleted-account"
            );
            assert!(f
                .seed
                .store
                .trigger_by_public_id(&f.public.public_id)
                .await
                .unwrap()
                .is_none());
            return;
        }
    }
    panic!("publisher record erasure did not finish");
}

async fn pending() -> (Arc<Fixture>, Launch, AgentAdmission) {
    let f = Arc::new(continuity_fixture().await);
    let launch = f
        .ready_for_agent(f.launch("late-cached-agent-admission").await)
        .await;
    let request = super::checkpoint_tests::admission_with_message(&f, &launch).await;
    (f, launch, request)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_refuses_cached_new_agent_admission_even_when_profile_pins_survive() {
    let (f, launch, request) = pending().await;
    erase_publisher_records(&f).await;
    assert!(
        f.resources
            .admit_catalogue_agent_with_trigger_initiator(
                request.clone(),
                launch.initiation.unwrap(),
                false,
            )
            .await
            .is_err(),
        "a retained Profile pin must not authorize a deleted publisher's cached Launch"
    );
    assert!(f
        .resources
        .get_agent(&request.agent_id)
        .await
        .unwrap()
        .is_none());
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_keeps_exact_committed_agent_admission_replay_available() {
    let (f, launch, request) = pending().await;
    let initiation = launch.initiation.unwrap();
    let accepted = f
        .resources
        .admit_catalogue_agent_with_trigger_initiator(request.clone(), initiation.clone(), false)
        .await
        .unwrap();
    erase_publisher_records(&f).await;
    let replay = f
        .resources
        .admit_catalogue_agent_with_trigger_initiator(request, initiation, false)
        .await
        .unwrap();
    assert_eq!(replay.creation().agent, accepted.creation().agent);
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_fences_an_agent_transaction_started_before_definition_deletion() {
    let (f, launch, request) = pending().await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let notice = Arc::new(tokio::sync::Notify::new());
    let notify = notice.clone();
    let (release, receiver) = mpsc::channel();
    let receiver = Mutex::new(receiver);
    let fired = AtomicBool::new(false);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            // The aggregate read already established the old transaction's
            // snapshot; pause its next read before it acquires writer fences.
            if event.command.get_str("find").ok() == Some("trigger_launch_input_protocols")
                && !fired.swap(true, Ordering::SeqCst)
            {
                notify.notify_one();
                receiver
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = crate::store::MongoSandboxStore::open(&client, &f.seed.database)
        .await
        .unwrap();
    let expected_id = request.agent_id.clone();
    let task = tokio::spawn(async move {
        store
            .admit_catalogue_agent_with_trigger_initiator(
                request,
                launch.initiation.unwrap(),
                false,
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    erase_publisher_records(&f).await;
    release.send(()).unwrap();
    assert!(
        task.await.unwrap().is_err(),
        "an old transaction must serialize with publisher erasure"
    );
    assert!(f.resources.get_agent(&expected_id).await.unwrap().is_none());
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}
