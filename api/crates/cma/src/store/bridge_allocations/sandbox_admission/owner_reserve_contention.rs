//! Actual Mongo conflicts prove ordinary and Channel writers share one fence.
use super::*;
use crate::store::sandbox_mongo::{is_transient, start_transaction};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    error::{ErrorKind, WriteFailure},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::{collections::BTreeSet, future::Future, sync::Mutex};
use tokio::sync::{mpsc, oneshot, watch};

async fn monitored_owner_writer(database: &str, owner: &str) -> (Client, oneshot::Receiver<()>) {
    let (notice, conflict) = oneshot::channel();
    let pending = Mutex::new((BTreeSet::<i32>::new(), Some(notice)));
    let database = database.to_owned();
    let owner = owner.to_owned();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut pending = pending.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if event.db == database
                    && event.command.get_str("update").ok() == Some("sandbox_admissions")
                    && event.command.get_bool("autocommit").ok() == Some(false)
                    && event.command.get_array("updates").is_ok_and(|updates| {
                        updates.iter().any(|update| {
                            update
                                .as_document()
                                .and_then(|update| update.get_document("q").ok())
                                .and_then(|query| query.get_str("_id").ok())
                                == Some(owner.as_str())
                        })
                    }) =>
            {
                pending.0.insert(event.request_id);
            }
            CommandEvent::Failed(event) => {
                let write_conflict = match event.failure.kind.as_ref() {
                    ErrorKind::Command(error) => error.code == 112,
                    ErrorKind::Write(WriteFailure::WriteError(error)) => error.code == 112,
                    _ => false,
                };
                if pending.0.remove(&event.request_id)
                    && write_conflict
                    && is_transient(&event.failure)
                {
                    if let Some(notice) = pending.1.take() {
                        let _ = notice.send(());
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                pending.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    (Client::with_options(options).unwrap(), conflict)
}

async fn resume_after_conflict<F: Future>(
    operation: F,
    conflict: oneshot::Receiver<()>,
    reached: mpsc::UnboundedSender<()>,
    mut release: watch::Receiver<bool>,
) -> F::Output {
    tokio::pin!(operation);
    tokio::select! {
        biased;
        observed = conflict => observed.expect("observe an actual owner fence write conflict"),
        _ = &mut operation => panic!("admission completed while the owner fence was held"),
    }
    reached.send(()).unwrap();
    // Freeze this same production future after its first actual Mongo conflict
    // while slower contenders reach the fence. No retry budget is spent waiting
    // for the coordinator, and the original authority deadline is not extended.
    release.wait_for(|released| *released).await.unwrap();
    operation.await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn eligible_channel_manual_and_trigger_admissions_conflict_on_same_owner_fence() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let initial = request(&svc, &bridges, &id, &allocations, "initial").await;
    allocations
        .admit_channel_sandbox(initial.clone())
        .await
        .unwrap();
    let owner = initial.sandbox().owner.as_str();
    let (channel_client, channel_conflict) = monitored_owner_writer(&database, owner).await;
    let (manual_client, manual_conflict) = monitored_owner_writer(&database, owner).await;
    let (trigger_client, trigger_conflict) = monitored_owner_writer(&database, owner).await;
    let channel_bridges = Arc::new(open_store(&channel_client, &database).await);
    let channel = MongoBridgeAllocationStore::open(channel_bridges, None, &database)
        .await
        .unwrap();
    let manual = MongoSandboxStore::open(&manual_client, &database)
        .await
        .unwrap();
    let trigger = MongoSandboxStore::open(&trigger_client, &database)
        .await
        .unwrap();
    // Prepare fresh authority after opening the monitored clients, preserving
    // the fixture's real 30-second deadline through the bounded contention.
    let candidate = request(&svc, &bridges, &id, &allocations, "eligible").await;
    assert_eq!(candidate.live_limit(), 3);
    assert_eq!(
        resources.sandboxes.count_documents(doc! {}).await.unwrap(),
        1
    );
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    assert_eq!(
        resources
            .sandbox_admissions
            .update_one(
                doc! {"_id":owner},
                doc! {"$inc":{"owned_reserve_contention_fixture":1_i64}},
            )
            .session(&mut holder)
            .await
            .unwrap()
            .matched_count,
        1
    );
    let (reached, mut notices) = mpsc::unbounded_channel();
    let (release, released) = watch::channel(false);
    let channel_attempt = resume_after_conflict(
        channel.admit_channel_sandbox(candidate.clone()),
        channel_conflict,
        reached.clone(),
        released.clone(),
    );
    let manual_attempt = resume_after_conflict(
        manual.admit_sandbox(ordinary(&initial, "manual-contender", false)),
        manual_conflict,
        reached.clone(),
        released.clone(),
    );
    let trigger_attempt = resume_after_conflict(
        trigger.admit_sandbox(ordinary(&initial, "trigger-contender", true)),
        trigger_conflict,
        reached,
        released,
    );
    let coordinator = async {
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..3 {
                notices
                    .recv()
                    .await
                    .expect("each contender reaches the owner fence");
            }
        })
        .await;
        let live_while_held = resources.sandboxes.count_documents(doc! {}).await.unwrap();
        holder.abort_transaction().await.unwrap();
        release.send(true).unwrap();
        observed.expect("all three admissions must hit the held owner fence");
        assert_eq!(live_while_held, 1);
    };
    let (channel_result, manual_result, trigger_result, ()) =
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                channel_attempt,
                manual_attempt,
                trigger_attempt,
                coordinator
            )
        })
        .await
        .expect("released admissions finish within their original authority deadline");
    let channel_created = match channel_result {
        Ok(receipt) => {
            assert!(receipt.created);
            true
        }
        Err(AllocationError::Capacity) => {
            untouched(&allocations, &resources, &candidate).await;
            false
        }
        Err(other) => panic!("unexpected Channel race result: {other:?}"),
    };
    let mut ordinary_created = 0;
    for result in [manual_result, trigger_result] {
        match result {
            Ok(SandboxAdmissionResult::Created(_)) => ordinary_created += 1,
            Err(SandboxStoreError::LimitReached { limit: 3, live: 3 }) => {}
            other => panic!("unexpected ordinary race result: {other:?}"),
        }
    }
    // If Channel commits first, one ordinary writer gets the reserved slot.
    // If an ordinary writer commits first, Channel refuses and both ordinary
    // writers fit. Any other outcome violates the serialized quota boundary.
    assert_eq!(ordinary_created, if channel_created { 1 } else { 2 });
    assert_eq!(
        resources.sandboxes.count_documents(doc! {}).await.unwrap(),
        3
    );
    assert_eq!(
        resources
            .sandbox_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        resources
            .resource_policies
            .count_documents(doc! {})
            .await
            .unwrap(),
        1 + u64::from(channel_created)
    );
    client.database(&database).drop().await.unwrap();
}
