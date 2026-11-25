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
