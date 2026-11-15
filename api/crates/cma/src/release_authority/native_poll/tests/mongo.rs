//! Store mechanics against an owned replica set; no provider or live-finality
//! qualification. Explicitly ignored until the fixture owner admits this slice.
use super::*;
use crate::release_authority::{
    tests::{bootstrap, transition, writer, TestEvidence},
    COLLECTIONS, HEADS, STORE_COMMANDS,
};
use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    Client, Database,
};

mod races;

async fn fixture() -> (Database, AuthorityStore) {
    let uri =
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable local replica set required");
    assert!(uri.starts_with("mongodb://127.0.0.1:") || uri.starts_with("mongodb://localhost:"));
    let client = Client::with_uri_str(uri).await.unwrap();
    let db = client.database(&format!(
        "cma_native_poll_{}",
        uuid::Uuid::new_v4().simple()
    ));
    for collection in COLLECTIONS {
        db.create_collection(*collection).await.unwrap();
    }
    db.create_collection("native_warm_pool_targets")
        .await
        .unwrap();
    let store = AuthorityStore::open(client, db.clone()).await.unwrap();
    store.bootstrap(&bootstrap(), &TestEvidence).await.unwrap();
    (db, store)
}

async fn prepare(
    store: &AuthorityStore,
    previous: Option<&NativePollReceipt>,
) -> NativePollCommand {
    store
        .prepare_native_poll(&writer(1, 1), NativePollLane::Background, previous)
        .await
        .unwrap()
}

fn fresh(result: NativePollExecution) -> NativePollReceipt {
    let NativePollExecution::Fresh(receipt) = result else {
        panic!("fresh acknowledged transaction")
    };
    receipt
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn empty_poll_advances_without_effect_permit_and_replays_without_rescan() {
    let (db, store) = fixture().await;
    let command = prepare(&store, None).await;
    let receipt = fresh(store.execute_native_poll(&command).await.unwrap());
    assert!(matches!(receipt.outcome(), NativePollOutcome::Empty));
    store
        .native_targets
        .insert_one(testing::target())
        .await
        .unwrap();
    let NativePollExecution::Replay(replay) = store.execute_native_poll(&command).await.unwrap()
    else {
        panic!("historical result must replay")
    };
    assert_eq!(replay.sha256(), receipt.sha256());
    assert!(matches!(replay.outcome(), NativePollOutcome::Empty));
    assert!(store
        .native_targets
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap()
        .lease
        .is_none());
    assert!(matches!(
        store
            .prepare_native_poll(&writer(1, 1), NativePollLane::Background, None)
            .await,
        Err(AuthorityError::Conflict)
    ));
    let next = prepare(&store, Some(&receipt)).await;
    assert_eq!(next.sequence(), 2);
    let positive = fresh(store.execute_native_poll(&next).await.unwrap());
    assert!(matches!(positive.outcome(), NativePollOutcome::Claimed(_)));
    let head = store.load("test-deployment").await.unwrap().unwrap();
    assert_eq!(head.admission_sequence, 2);
    assert_eq!(head.active_effects, 0);
    assert_eq!(head.control_revision, 1);
    assert_eq!(store.effects.count_documents(doc! {}).await.unwrap(), 0);
    // Original receipt remains byte-identical even after cursor advancement.
    assert_eq!(
        store
            .reconcile_native_poll(&command)
            .await
            .unwrap()
            .unwrap()
            .sha256(),
        receipt.sha256()
    );
    assert_eq!(
        store
            .native_poll_tip(&writer(1, 1), NativePollLane::Background)
            .await
            .unwrap()
            .unwrap()
            .sha256(),
        positive.sha256()
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn positive_replay_preserves_original_lease_and_receipt_after_target_changes() {
    let (db, store) = fixture().await;
    let target = testing::target();
    store
        .native_targets
        .insert_one(target.clone())
        .await
        .unwrap();
    let command = prepare(&store, None).await;
    let receipt = fresh(store.execute_native_poll(&command).await.unwrap());
    let original = crate::release_authority::store::document(&receipt).unwrap();
    store
        .native_targets
        .update_one(
            doc! {"_id": &target.id},
            doc! {"$set": {
                "revision": 2_i64, "lease": Bson::Null, "lease_until": DateTime::from_millis(0),
            }},
        )
        .await
        .unwrap();
    let NativePollExecution::Replay(replay) = store.execute_native_poll(&command).await.unwrap()
    else {
        panic!("positive replay must never be Fresh")
    };
    assert_eq!(
        crate::release_authority::store::document(&replay).unwrap(),
        original
    );
    let NativePollOutcome::Claimed(claimed) = replay.outcome() else {
        panic!("retained target")
    };
    assert_eq!(claimed.revision, 1);
    assert_eq!(claimed.lease.as_deref(), Some(lease(&command.0).as_str()));
    assert!(store
        .native_targets
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap()
        .lease
        .is_none());
    assert_eq!(
        store
            .store_commands
            .count_documents(doc! {"kind": "native_target_poll_receipt"})
            .await
            .unwrap(),
        1
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn malformed_target_aborts_lease_head_cursor_and_receipt_together() {
    let (db, store) = fixture().await;
    let mut target = testing::target();
    target.applied_revision = target.revision + 1;
    store
        .native_targets
        .insert_one(target.clone())
        .await
        .unwrap();
    let command = prepare(&store, None).await;
    assert!(matches!(
        store.execute_native_poll(&command).await,
        Err(AuthorityError::Invalid)
    ));
    assert!(store
        .reconcile_native_poll(&command)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .native_targets
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap()
        .lease
        .is_none());
    assert_eq!(
        store.store_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    assert_eq!(
        store
            .load("test-deployment")
            .await
            .unwrap()
            .unwrap()
            .admission_sequence,
        0
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn expired_non_null_lease_is_never_taken_over() {
    let (db, store) = fixture().await;
    let mut target = testing::target();
    target.lease = Some("unresolved-original".into());
    target.lease_until = DateTime::from_millis(1);
    store.native_targets.insert_one(target).await.unwrap();
    let command = prepare(&store, None).await;
    assert!(matches!(
        fresh(store.execute_native_poll(&command).await.unwrap()).outcome(),
        NativePollOutcome::Empty
    ));
    let target = store
        .native_targets
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target.lease.as_deref(), Some("unresolved-original"));
    assert_eq!(target.lease_until, DateTime::from_millis(1));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn changed_command_and_missing_observation_cannot_skip_sequence() {
    let (db, store) = fixture().await;
    let command = prepare(&store, None).await;
    // Missing receipt is merely unknown; it never fabricates a completed poll.
    assert!(store
        .reconcile_native_poll(&command)
        .await
        .unwrap()
        .is_none());
    let mut changed = command.clone();
    changed.0.sequence = 2;
    changed.0.key = command_key(&changed.0.writer, changed.0.lane, 2).unwrap();
    changed.0.previous_receipt_sha256 = Some("b".repeat(64));
    assert!(matches!(
        store.execute_native_poll(&changed).await,
        Err(AuthorityError::Conflict)
    ));
    let first = fresh(store.execute_native_poll(&command).await.unwrap());
    changed = command.clone();
    changed.0.writer.instance_id.push_str("-foreign");
    changed.0.key = command_key(&changed.0.writer, changed.0.lane, 1).unwrap();
    assert!(matches!(
        store.execute_native_poll(&changed).await,
        Err(AuthorityError::Closed)
    ));
    let (_, foreign) = testing::fixture(NativePollLane::StartupAgent, None);
    assert!(matches!(
        store
            .prepare_native_poll(&writer(1, 1), NativePollLane::Background, Some(&foreign))
            .await,
        Err(AuthorityError::Conflict)
    ));
    assert_eq!(prepare(&store, Some(&first)).await.sequence(), 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires owned CHRONO_TEST_MONGODB_URI replica set"]
async fn head_one_refuses_without_migration_and_receipt_tampering_is_invalid() {
    let (db, store) = fixture().await;
    let command = prepare(&store, None).await;
    db.collection::<Document>(HEADS)
        .update_one(
            doc! {"_id": "test-deployment"},
            doc! {"$set": {"schema_version": 1_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.execute_native_poll(&command).await,
        Err(AuthorityError::Invalid)
    ));
    assert_eq!(
        store.store_commands.count_documents(doc! {}).await.unwrap(),
        0
    );
    // Explicit fixture-only restoration; production never auto-upgrades a head.
    db.collection::<Document>(HEADS)
        .update_one(
            doc! {"_id": "test-deployment"},
            doc! {"$set": {"schema_version": 2_i64}},
        )
        .await
        .unwrap();
    let receipt = fresh(store.execute_native_poll(&command).await.unwrap());
    db.collection::<Document>(STORE_COMMANDS)
        .update_one(
            doc! {"_id": receipt.key()},
            doc! {"$set": {"admission_sequence": 900_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.reconcile_native_poll(&command).await,
        Err(AuthorityError::Invalid)
    ));
    db.drop().await.unwrap();
}
