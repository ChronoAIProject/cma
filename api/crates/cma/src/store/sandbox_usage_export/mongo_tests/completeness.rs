use super::*;
use crate::sandbox_usage_export::completeness_contract::{Basis, Statement};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_outbox_binds_the_actual_projection_and_keeps_retry_bytes() {
    let mut fixture = Fixture::new(true).await;
    let mut enabled = config();
    enabled.completeness_enabled = true;
    fixture.store = fixture
        .store
        .clone()
        .with_usage_capture(Some(enabled.clone()))
        .await
        .unwrap();
    let parent = fixture.parent("unfunded-owner").await;
    let capture = fixture.capture();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let id = source_id(&parent, 2);
    let (first, second) = tokio::join!(
        capture.project_one(&id, now()),
        capture.project_one(&id, now())
    );
    assert_eq!(
        [first.unwrap(), second.unwrap()]
            .into_iter()
            .filter(|success| *success)
            .count(),
        1
    );
    let streams = capture.completeness_source_page("").await.unwrap();
    assert_eq!(streams.len(), 1);
    assert!(capture.backfill_completeness(&streams[0]).await.unwrap());
    let pending = capture.completeness_delivery_page("").await.unwrap();
    assert_eq!(pending.len(), 1);
    let destination = "a".repeat(64);
    let attempt = capture
        .claim_completeness(&pending[0], &destination)
        .await
        .unwrap()
        .unwrap();
    let statement =
        Statement::from_value(serde_json::from_slice(&attempt.payload).unwrap()).unwrap();
    assert_eq!(statement.basis, Basis::Allocated);
    assert_eq!(statement.sandbox_id, parent.id.as_str());
    assert_eq!(statement.timeline.len(), 1);
    assert_eq!(statement.facts.len(), 1);
    let actual = fixture
        .db
        .collection::<Document>(OUTBOX)
        .find_one(doc! {"_id":&statement.facts[0].event_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        actual.get_str("digest").unwrap(),
        statement.facts[0].event_digest
    );
    assert_eq!(
        statement.positions[0].digest,
        statement.facts[0].event_digest
    );
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), enabled)
        .await
        .unwrap();
    assert!(!reopened.project_one(&id, now() + 1).await.unwrap());
    assert_eq!(
        reopened.completeness_delivery_page("").await.unwrap(),
        pending
    );
    assert!(reopened
        .claim_completeness(&pending[0], &destination)
        .await
        .unwrap()
        .is_none());
    let receipt = CompletenessReceipt {
        statement_id: attempt.id.clone(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        statement_digest: attempt.digest.clone(),
        accepted_at_ms: (now() * 1000).to_string(),
        custody: "persisted".into(),
    };
    reopened
        .finish_completeness(&attempt, CompletenessResult::Accepted(receipt))
        .await
        .unwrap();
    assert!(reopened
        .claim_completeness(&pending[0], &destination)
        .await
        .unwrap()
        .is_none());
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_backfills_retained_minutes_and_serializes_with_new_capture() {
    let mut fixture = Fixture::new(true).await;
    let parent = fixture.parent("retained-owner").await;
    let old = fixture.capture();
    old.project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    old.project_one(&source_id(&parent, 2), now())
        .await
        .unwrap();
    assert!(old.completeness_delivery_page("").await.unwrap().is_empty());
    let mut enabled = config();
    enabled.completeness_enabled = true;
    fixture.store = fixture
        .store
        .clone()
        .with_usage_capture(Some(enabled))
        .await
        .unwrap();
    let capture = fixture.capture();
    let streams = capture.completeness_source_page("").await.unwrap();
    let stream = &streams[0];
    let (published, checkpoint) = tokio::join!(
        capture.backfill_completeness(stream),
        fixture
            .store
            .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
    );
    if checkpoint.is_err() {
        fixture
            .store
            .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
            .await
            .unwrap();
    }
    capture
        .project_one(&source_id(&parent, 3), now())
        .await
        .unwrap();
    // A racing capture may postpone publication. Each closed minute is emitted
    // exactly once, from the actual retained source after the writers settle.
    let _ = published;
    while capture.backfill_completeness(stream).await.unwrap() {}
    let pending = capture.completeness_delivery_page("").await.unwrap();
    assert_eq!(pending.len(), 2);
    let mut minutes = std::collections::BTreeSet::new();
    for id in pending {
        let attempt = capture
            .claim_completeness(&id, &"a".repeat(64))
            .await
            .unwrap()
            .unwrap();
        let statement =
            Statement::from_value(serde_json::from_slice(&attempt.payload).unwrap()).unwrap();
        assert_eq!(statement.timeline.len(), 1);
        assert_eq!(statement.revision.0, 1);
        minutes.insert(statement.minute_start_ms);
    }
    assert_eq!(minutes.len(), 2);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_pending_fragment_survives_progress_and_retry_after_restart() {
    let mut fixture = Fixture::new(true).await;
    let mut enabled = config();
    enabled.completeness_enabled = true;
    fixture.store = fixture
        .store
        .clone()
        .with_usage_capture(Some(enabled.clone()))
        .await
        .unwrap();
    let parent = fixture.parent("retained-fixture").await;
    let capture = fixture.capture();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    capture
        .project_one(&source_id(&parent, 2), now())
        .await
        .unwrap();
    let stream = capture
        .completeness_source_page("")
        .await
        .unwrap()
        .remove(0);
    // This synthetic database models a projected fragment temporarily absent
    // from retained custody, then repaired with its original immutable bytes.
    let events = fixture.db.collection::<Document>(OUTBOX);
    let missing = events
        .find_one_and_delete(doc! {"source_id":source_id(&parent,2),"part":0_i64})
        .await
        .unwrap()
        .unwrap();
    assert!(capture.backfill_completeness(&stream).await.unwrap());
    let outcomes = fixture.db.collection::<Document>(INVENTORY);
    let pending = outcomes
        .find_one(doc! {"_id.usage_completeness_outcome_v1":{"$exists":true}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.get_str("status").unwrap(), "pending");
    assert_eq!(
        pending.get_str("reason").unwrap(),
        "projected_event_missing"
    );
    assert!(capture
        .completeness_delivery_page("")
        .await
        .unwrap()
        .is_empty());
    events.insert_one(missing).await.unwrap();
    outcomes
        .update_many(
            doc! {"_id.usage_completeness_retry_v1":{"$exists":true}},
            doc! {"$set":{"due_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), enabled)
        .await
        .unwrap();
    assert!(reopened.backfill_completeness(&stream).await.unwrap());
    let outcome = outcomes
        .find_one(doc! {"_id":pending.get_document("_id").unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.get_str("status").unwrap(), "staged");
    assert_eq!(outcome.get_i64("attempts").unwrap(), 2);
    assert_eq!(
        reopened.completeness_delivery_page("").await.unwrap().len(),
        1
    );
    assert!(!reopened.backfill_completeness(&stream).await.unwrap());
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_unproved_historical_scope_retains_held_outcome() {
    use futures_util::TryStreamExt;
    let mut fixture = Fixture::new(false).await;
    // A retained Sandbox predates the usage ledger. Its first captured
    // baseline establishes the runtime binding at a mid-minute boundary.
    let mut parent = sandbox(layer2_core::SandboxId::generate().as_str());
    parent.owner = layer2_core::SubjectId::new("historical-owner");
    parent.runtime = Some(RuntimeBinding {
        runtime_id: "runtime-fixture".into(),
        runtime_version: "image-fixture".into(),
        bound_at: NOW.into(),
        boot_id: None,
        provision_generation: Some(1),
    });
    parent.lifecycle.runtime_generation_high_water = 1;
    fixture.store.create_sandbox(parent.clone()).await.unwrap();
    fixture.prepare().await;
    let mut enabled = config();
    enabled.completeness_enabled = true;
    fixture.store = fixture
        .store
        .with_usage_capture(Some(enabled.clone()))
        .await
        .unwrap();
    let capture = fixture.capture();
    capture
        .baseline(&fixture.store, "2026-09-05T00:00:30Z", 1)
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let rows = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find(doc! {})
        .sort(doc! {"revision":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert!(capture
            .project_one(row.get_str("_id").unwrap(), now())
            .await
            .unwrap());
    }
    let stream = capture
        .completeness_source_page("")
        .await
        .unwrap()
        .remove(0);
    assert!(capture.backfill_completeness(&stream).await.unwrap());
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), enabled)
        .await
        .unwrap();
    let outcome = fixture
        .db
        .collection::<Document>(INVENTORY)
        .find_one(doc! {"_id.usage_completeness_outcome_v1":{"$exists":true}})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.get_str("status").unwrap(), "held");
    assert_eq!(outcome.get_str("reason").unwrap(), "unproved_scope_start");
    assert!(!reopened.backfill_completeness(&stream).await.unwrap());
    assert!(reopened
        .completeness_delivery_page("")
        .await
        .unwrap()
        .is_empty());
    fixture.db.drop().await.unwrap();
}
