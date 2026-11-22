//! A Profile read racing publisher erasure must report the publisher boundary.
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

async fn erase_unreferenced_publisher(f: &Fixture) {
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
            for collection in [
                "sandbox_profile_revisions",
                "workspace_profile_revisions",
                "agent_profile_revisions",
            ] {
                assert_eq!(
                    db.collection::<Document>(collection)
                        .count_documents(doc! {})
                        .await
                        .unwrap(),
                    0
                );
            }
            assert!(f
                .seed
                .store
                .trigger_by_public_id(&f.public.public_id)
                .await
                .unwrap()
                .is_none());
            assert!(
                f.seed
                    .store
                    .trigger_revision(&f.public.trigger_id, &f.public.revision.digest)
                    .await
                    .unwrap()
                    .unwrap()
                    .publisher_deleted
            );
            return;
        }
    }
    panic!("unreferenced publisher erasure did not finish");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_profile_read_keeps_ordinary_missing_profile_failure_strict() {
    let f = Arc::new(Fixture::new(false).await);
    let launch = f.launch("ordinary-missing-frozen-profile").await;
    let db = f.seed.client.database(&f.seed.database);
    assert_eq!(
        db.collection::<Document>("sandbox_profile_revisions")
            .delete_one(doc! {
                "profile_id":f.seed.target.sandbox_profile.id.as_str(),
                "revision":i64::from(f.seed.target.sandbox_profile.revision),
            })
            .await
            .unwrap()
            .deleted_count,
        1
    );
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("ordinary-missing-profile-worker"),
    );
    assert!(worker
        .run_one(&launch.id, std::time::SystemTime::now())
        .await
        .unwrap());
    let settled = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, LaunchState::FailedTerminal);
    assert_eq!(settled.failure.unwrap().code, "frozen_profile_missing");
    assert!(
        !f.seed
            .store
            .trigger_revision(&launch.trigger_id, &launch.revision_digest)
            .await
            .unwrap()
            .unwrap()
            .publisher_deleted
    );
    assert!(f
        .seed
        .store
        .trigger(&launch.trigger_id, &f.seed.owner)
        .await
        .unwrap()
        .is_some());
    db.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_erasure_racing_a_frozen_profile_read_settles_typed_retired() {
    let mut f = Fixture::new(false).await;
    let launch = f.launch("erasure-racing-frozen-profile").await;
    assert!(launch.resolution.sandbox.is_pending());
    assert!(launch.resolution.agent.is_pending());
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let notice = Arc::new(tokio::sync::Notify::new());
    let notify = notice.clone();
    let (release, receiver) = mpsc::channel();
    let receiver = Mutex::new(receiver);
    let fired = AtomicBool::new(false);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            // The worker has already read the ordinary immutable Trigger revision.
            // Pause its first exact Profile lookup before the command reaches Mongo.
            if event.command.get_str("find").ok() == Some("sandbox_profile_revisions")
                && event
                    .command
                    .get_document("filter")
                    .is_ok_and(|filter| filter.contains_key("profile_id"))
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
    f.seed.profiles = Arc::new(
        crate::store::MongoProfileStore::open(&client, &f.seed.database)
            .await
            .unwrap(),
    );
    let f = Arc::new(f);
    let worker = LaunchWorker::new(
        Arc::new(f.seed.store.clone()),
        Arc::new(Driver(f.clone())),
        LaunchWorkerConfig::new("erasure-racing-profile-worker"),
    );
    let id = launch.id.clone();
    let task = tokio::spawn(async move { worker.run_one(&id, std::time::SystemTime::now()).await });
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    erase_unreferenced_publisher(&f).await;
    release.send(()).unwrap();
    assert!(task.await.unwrap().unwrap());
    let settled = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled.state, LaunchState::FailedTerminal);
    assert!(settled.resolution.sandbox.is_pending());
    assert!(settled.resolution.agent.is_pending());
    let failure = settled.failure.unwrap();
    assert_eq!(failure.code, "trigger_retired");
    assert!(failure.safe_detail.contains("another Trigger"));
    f.seed
        .client
        .database(&f.seed.database)
        .drop()
        .await
        .unwrap();
}
