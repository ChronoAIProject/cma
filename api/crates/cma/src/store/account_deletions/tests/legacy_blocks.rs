//! Fixed C1 wire shapes must remain readable after their executor is replaced.
use super::*;
use layer2_core::account_deletion::ErasureEventKind;
use mongodb::bson::{self, Bson};
use std::collections::BTreeMap;

const ID: &str = "acd_11111111111111111111111111111111";
const OWNER: &str = "historical-erasure-owner";
const KEY: &str = "historical-erasure-request";

fn reason(kind: &str) -> Document {
    doc! {"kind":kind,"collection":"agent_profiles"}
}

// These field names, omitted optional fields, integer types and event nesting
// match C1 f862568e0 record_document/event_document, independently of the reader
// under test. Four claims and three completed steps precede the blocked event.
fn c1_record(block: Document) -> Document {
    let steps: Vec<_> = ["inventory", "retire_sandboxes", "release_homes"]
        .into_iter()
        .map(|step| doc! {"step":step,"completed_at":NOW,"counts":{}})
        .collect();
    let hash = subject_hash(&SubjectId::new(OWNER));
    doc! {
        "_id":ID,"document_schema_version":1_i64,
        "request_keys":[key_hash(KEY)],"subject":OWNER,
        "subject_hash":&hash,"active_subject_hash":hash,
        "status":"blocked","step":"erase_records","block":block,
        "requested_at":NOW,"deadline_at":"2026-11-21T00:00:00Z",
        "work":{"ready_at":NOW,"attempts":0_i64,"next_fence":5_i64},
        "inventory":{"sandboxes":[],"workspaces":[],"worktrees":[],
            "agents":[],"launches":[],"bindings":[],"triggers":[],
            "bots":[],"documents":[],"homes":[],"pending_key_ids":[],"counts":{}},
        "revision":9_i64,"event_seq":9_i64,"steps":steps,
    }
}

fn c1_events(block: Document) -> Vec<Document> {
    let mut events = Vec::new();
    for (seq, step, kind) in [
        (1_i64, "inventory", doc! {"kind":"started"}),
        (2, "inventory", doc! {"kind":"started"}),
        (3, "inventory", doc! {"kind":"completed","counts":{}}),
        (4, "retire_sandboxes", doc! {"kind":"started"}),
        (5, "retire_sandboxes", doc! {"kind":"completed","counts":{}}),
        (6, "release_homes", doc! {"kind":"started"}),
        (7, "release_homes", doc! {"kind":"completed","counts":{}}),
        (8, "erase_records", doc! {"kind":"started"}),
        (9, "erase_records", doc! {"kind":"blocked","reason":block}),
    ] {
        events.push(doc! {
            "_id":format!("{ID}:{seq:020}"),"document_schema_version":1_i64,
            "deletion_id":ID,"seq":seq,"at":NOW,"step":step,"kind":kind,
        });
    }
    events
}

async fn raw_record(db: &Database) -> Document {
    db.collection::<Document>(RECORDS)
        .find_one(doc! {"_id":ID})
        .await
        .unwrap()
        .unwrap()
}

async fn raw_events(db: &Database) -> Vec<Document> {
    db.collection::<Document>(EVENTS)
        .find(doc! {"deletion_id":ID})
        .sort(doc! {"seq":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

fn encoded(value: &impl serde::Serialize) -> Document {
    bson::serialize_to_document(value).unwrap()
}

async fn persisted_block_replays_and_resumes(kind: &str) {
    let (_, db, store) = fixture().await;
    let block = reason(kind);
    let original = c1_record(block.clone());
    let history = c1_events(block.clone());
    store.records.insert_one(original.clone()).await.unwrap();
    store.events.insert_many(history.clone()).await.unwrap();
    let owner = SubjectId::new(OWNER);
    let id = AccountDeletionId(ID.into());
    let loaded = store.get(&id).await.unwrap().unwrap();
    assert_eq!(loaded.status, AccountDeletionStatus::Blocked);
    assert_eq!(encoded(loaded.block.as_ref().unwrap()), block);
    assert_eq!(
        record_document(&loaded, &[key_hash(KEY)]).unwrap(),
        original
    );
    for record in [
        store.active(&owner).await.unwrap().unwrap(),
        store.latest(&owner).await.unwrap().unwrap(),
        store.replay(&owner, KEY).await.unwrap().unwrap(),
        store.request(&owner, KEY, LATER).await.unwrap(),
    ] {
        assert_eq!(record, loaded);
    }
    assert!(store
        .replay(&owner, "different-request")
        .await
        .unwrap()
        .is_none());
    assert!(store
        .replay(&SubjectId::new("another-owner"), KEY)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_due("worker", LATER, Duration::from_secs(90))
        .await
        .unwrap()
        .is_none());
    assert_eq!(raw_record(&db).await, original);
    assert_eq!(raw_events(&db).await, history);

    let at = database_now(&db).await;
    let mut rearmed = loaded.clone();
    let event = rearmed.rearm(&at).unwrap();
    store
        .commit(rearmed, vec![event], loaded.revision, None)
        .await
        .unwrap();
    let mut claimed = store
        .claim_due("compatible-worker", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.step, ErasureStep::EraseRecords);
    assert!(claimed.block.is_none());
    let revision = claimed.revision;
    let lease = claimed.work.lease.as_ref().unwrap().clone();
    let event = claimed.advance(lease.fence, &at, BTreeMap::new()).unwrap();
    store
        .commit(claimed, vec![event], revision, Some(&lease.token))
        .await
        .unwrap();
    let resumed = store.get(&id).await.unwrap().unwrap();
    assert_eq!(resumed.status, AccountDeletionStatus::Running);
    assert_eq!(resumed.step, ErasureStep::EraseObjects);
    assert!(resumed.block.is_none());
    assert_eq!(store.replay(&owner, KEY).await.unwrap(), Some(resumed));
    let events = store.events(&id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=12).collect::<Vec<_>>()
    );
    let ErasureEventKind::Blocked { reason } = &events[8].kind else {
        panic!("the historical block was lost");
    };
    assert_eq!(encoded(reason), block);
    assert_eq!(&raw_events(&db).await[..history.len()], history.as_slice());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_block_record_replays_and_resumes() {
    persisted_block_replays_and_resumes("retained_reference_pending").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_identity_block_record_replays_and_resumes() {
    persisted_block_replays_and_resumes("identity_redaction_pending").await;
}

async fn historical_event_remains_readable(kind: &str) {
    let (_, db, store) = fixture().await;
    let history = c1_events(reason(kind));
    // The current aggregate no longer carries any legacy block. Event reading
    // must independently retain the older enum format.
    let mut current = c1_record(reason(kind));
    current.insert("status", "running");
    current.remove("block");
    store.records.insert_one(current).await.unwrap();
    store.events.insert_many(history.clone()).await.unwrap();
    let id = AccountDeletionId(ID.into());
    assert!(store.get(&id).await.unwrap().unwrap().block.is_none());
    let events = store.events(&id).await.unwrap();
    assert_eq!(events.len(), 9);
    assert_eq!(
        events
            .iter()
            .map(|event| event_document(event, None).unwrap())
            .collect::<Vec<_>>(),
        history
    );
    let ErasureEventKind::Blocked { reason: block } = &events[8].kind else {
        panic!("the historical block was lost");
    };
    assert_eq!(encoded(block), reason(kind));
    assert_eq!(raw_events(&db).await, history);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_publisher_block_event_outlives_the_blocked_record() {
    historical_event_remains_readable("retained_reference_pending").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn c1_identity_block_event_outlives_the_blocked_record() {
    historical_event_remains_readable("identity_redaction_pending").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_legacy_blocks_remain_errors_without_rewriting_evidence() {
    let (_, db, store) = fixture().await;
    let id = AccountDeletionId(ID.into());
    let owner = SubjectId::new(OWNER);
    let mut malformed = vec![doc! {"kind":"unknown_pending","collection":"agent_profiles"}];
    for kind in ["retained_reference_pending", "identity_redaction_pending"] {
        malformed.push(doc! {"kind":kind});
        malformed.push(doc! {"kind":kind,"collection":7_i64});
        malformed.push(doc! {"kind":kind,"collection":Bson::Null});
    }
    for block in malformed {
        let original = c1_record(block.clone());
        store
            .records
            .replace_one(doc! {"_id":ID}, original.clone())
            .upsert(true)
            .await
            .unwrap();
        let event = c1_events(block).pop().unwrap();
        store
            .events
            .replace_one(doc! {"_id":event.get_str("_id").unwrap()}, event.clone())
            .upsert(true)
            .await
            .unwrap();
        assert!(matches!(
            store.get(&id).await,
            Err(AccountDeletionError::Store(_))
        ));
        assert!(matches!(
            store.replay(&owner, KEY).await,
            Err(AccountDeletionError::Store(_))
        ));
        assert!(matches!(
            store.events(&id).await,
            Err(AccountDeletionError::Store(_))
        ));
        assert_eq!(raw_record(&db).await, original);
        assert_eq!(raw_events(&db).await, [event]);
    }
    db.drop().await.unwrap();
}
