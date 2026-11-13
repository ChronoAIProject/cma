//! Deterministic interleavings at the real driver's command boundary.
use super::{tests::*, *};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use tokio::sync::Notify;

pub(super) async fn intercepted(
    command: &'static str,
    collection: &'static str,
) -> (Database, Database, Arc<Notify>, mpsc::Sender<()>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let notice = Arc::new(Notify::new());
    let notify = notice.clone();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let fired = AtomicBool::new(false);
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let matches = event.command.get_str(command).ok() == Some(collection)
                && (command != "find"
                    || event
                        .command
                        .get_document("hint")
                        .is_ok_and(|hint| *hint == doc! {"_id":1}));
            if matches && !fired.swap(true, Ordering::SeqCst) {
                notify.notify_one();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap();
            }
        }
    }));
    let name = format!("cma_erasure_race_{}", uuid::Uuid::new_v4().simple());
    let observed = Client::with_options(options).unwrap().database(&name);
    let independent = Client::with_uri_str(&uri).await.unwrap().database(&name);
    (observed, independent, notice, release_tx)
}
async fn run(db: Database) -> Result<Batch, Error> {
    let subject = SubjectId::new("a");
    let mut inventory = ErasureInventory::default();
    let mut cursor = Cursor::default();
    for _ in 0..1000 {
        let batch = erase_records(
            &db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            NOW,
        )
        .await?;
        super::progress::acknowledge(&mut inventory, &batch, "a");
        if !batch.has_more || !batch.pending_key_ids.is_empty() {
            return Ok(batch);
        }
    }
    panic!("interleaved erasure failed to converge");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_revision_change_cannot_drop_new_revoke_debt() {
    let (db, independent, notice, release) = intercepted("find", "sandboxes").await;
    sandbox(&independent, "a", "sbx_a").await;
    let task = tokio::spawn(run(db));
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent.collection::<Document>("sandboxes").update_one(doc! {"_id":"sbx_a"},doc! {"$set":{"revision":2_i64,"record.revision":2_i64,"record.credential.revoke_debt":[{"provider_key_id":"late-debt","queued_at":NOW}]}}).await.unwrap();
    release.send(()).unwrap();
    let batch = task.await.unwrap().unwrap();
    assert_eq!(
        count(&independent, "sandboxes").await,
        1,
        "a new key must be acknowledged before deleting its revision"
    );
    assert_eq!(
        batch.pending_key_ids.get("a"),
        Some(&vec!["late-debt".to_owned()])
    );
    independent.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_billing_claim_preserves_delivery_and_usage_authority() {
    let (db, independent, notice, release) = intercepted("delete", "sandbox_usage_events").await;
    sandbox(&independent, "a", "sbx_a").await;
    put(
        &independent,
        "sandbox_usage",
        doc! {"_id":"sbx_a","owner":"a"},
    )
    .await;
    put(
        &independent,
        "sandbox_usage_events",
        doc! {"_id":"event","owner":"a","sandbox_id":"sbx_a","billing_delivered":false},
    )
    .await;
    let task = tokio::spawn(run(db));
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent.collection::<Document>("sandbox_usage_events").update_one(doc! {"_id":"event"},doc! {"$set":{"billing_claim":"late-claim","billing_delivered":false,"billing_lease_until":999999_i64}}).await.unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        }))
    ));
    assert_eq!(count(&independent, "sandbox_usage_events").await, 1);
    assert_eq!(count(&independent, "sandbox_usage").await, 1);
    independent.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn partial_billing_race_returns_acknowledged_deletion_counts() {
    let (db, independent, notice, release) = intercepted("delete", "sandbox_usage_events").await;
    put(
        &independent,
        "sandbox_usage",
        doc! {"_id":"usage","owner":"a"},
    )
    .await;
    for id in ["safe", "claimed"] {
        put(
            &independent,
            "sandbox_usage_events",
            doc! {"_id":id,"owner":"a","billing_delivered":false},
        )
        .await;
    }
    let task = tokio::spawn(run(db));
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent
        .collection::<Document>("sandbox_usage_events")
        .update_one(
            doc! {"_id":"claimed"},
            doc! {"$set":{"billing_claim":"late-claim"}},
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    let result = task.await.unwrap();
    let Err(Error::Interrupted { error, batch }) = result else {
        panic!("partial success must return its typed block and counts: {result:?}");
    };
    assert!(matches!(
        *error,
        Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        })
    ));
    assert_eq!(batch.deleted.get("sandbox_usage_events"), Some(&1));
    assert!(batch.has_more && batch.waiting);
    assert_eq!(count(&independent, "sandbox_usage_events").await, 1);
    assert_eq!(count(&independent, "sandbox_usage").await, 1);
    independent.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn late_funding_revision_preserves_new_pending_state() {
    let (db, independent, notice, release) = intercepted("delete", "sandbox_billing_state").await;
    put(
        &independent,
        "sandbox_billing_state",
        doc! {
            "_id":"orphan","owner_id":"a","revision":1_i64,"pending":null,"windows":[]
        },
    )
    .await;
    put(
        &independent,
        "sandbox_usage",
        doc! {"_id":"usage","owner":"a"},
    )
    .await;
    let task = tokio::spawn(run(db));
    tokio::time::timeout(std::time::Duration::from_secs(20), notice.notified())
        .await
        .unwrap();
    independent
        .collection::<Document>("sandbox_billing_state")
        .update_one(
            doc! {"_id":"orphan"},
            doc! {"$set":{"revision":2_i64,"pending":"late-reservation"}},
        )
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(Error::Blocked(AttributedBlock {
            reason: ErasureBlock::BillingSettlementPending,
            ..
        }))
    ));
    assert_eq!(count(&independent, "sandbox_billing_state").await, 1);
    assert_eq!(count(&independent, "sandbox_usage").await, 1);
    independent.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn fresh_organization_descendants_are_protected_without_inventory() {
    let db = database().await;
    sandbox(&db, "org", "sbx_org").await;
    put(
        &db,
        "resource_policies",
        doc! {"_id":"sbx_org","owner_kind":"organization","owner_subject":"org"},
    )
    .await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_org","sandbox_id":"sbx_org","owner":"org"},
    )
    .await;
    put(
        &db,
        "agent_suggestions",
        doc! {"_id":"org-suggestion","agent_id":"agt_org","owner":"a"},
    )
    .await;
    put(
        &db,
        "agent_message_feedback",
        doc! {"_id":"org-feedback","agent_id":"agt_org","owner":"a"},
    )
    .await;
    sandbox(&db, "a", "sbx_personal").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_personal","sandbox_id":"sbx_personal","owner":"a"},
    )
    .await;
    put(
        &db,
        "agent_suggestions",
        doc! {"_id":"personal","agent_id":"agt_personal","owner":"a"},
    )
    .await;
    let counts = drain(&db, "a", &mut ErasureInventory::default()).await;
    assert_eq!(counts.get("agent_suggestions"), Some(&2));
    assert_eq!(count(&db, "agent_suggestions").await, 0);
    assert_eq!(count(&db, "agent_message_feedback").await, 0);
    assert_eq!(count(&db, "agents").await, 1);
    assert_eq!(count(&db, "sandboxes").await, 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn stale_inventory_cannot_erase_another_persons_live_sandbox() {
    let db = database().await;
    sandbox(&db, "b", "sbx_b").await;
    put(
        &db,
        "agents",
        doc! {"_id":"agt_b","sandbox_id":"sbx_b","owner":"b"},
    )
    .await;
    put(&db, "agent_log", doc! {"_id":"b-log","agent_id":"agt_b"}).await;
    let mut inventory = ErasureInventory {
        sandboxes: vec!["sbx_b".into()],
        agents: vec!["agt_b".into()],
        ..Default::default()
    };
    let counts = drain(&db, "a", &mut inventory).await;
    assert_eq!(counts.values().sum::<u64>(), 0);
    for name in ["sandboxes", "agents", "agent_log"] {
        assert_eq!(count(&db, name).await, 1, "{name}");
    }
    db.drop().await.unwrap();
}
