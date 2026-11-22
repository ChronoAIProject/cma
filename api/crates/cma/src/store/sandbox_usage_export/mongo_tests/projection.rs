use super::*;
use futures_util::TryStreamExt;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capture_disable_gap_is_visible_and_cannot_advance_the_stream() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("gap-owner").await;
    let capture = fixture.capture();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    let off = fixture
        .store
        .clone()
        .with_usage_capture(None)
        .await
        .unwrap();
    off.checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), TWO_MINUTES)
        .await
        .unwrap();
    let id = source_id(&parent, 3);
    assert!(!capture.project_one(&id, now()).await.unwrap());
    let held = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.get_str("projection_hold").unwrap(), "source_gap");
    assert_eq!(fixture.count(OUTBOX).await, 2);
    let other = fixture.parent("other-owner").await;
    assert!(capture
        .project_one(&source_id(&other, 1), now())
        .await
        .unwrap());
    assert_eq!(fixture.count(OUTBOX).await, 4);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn source_projection_retries_keep_exact_frozen_bytes_and_stream_epoch() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("no-wallet").await;
    let capture = fixture.capture();
    let id = source_id(&parent, 1);
    let (a, b) = tokio::join!(
        capture.project_one(&id, now()),
        capture.project_one(&id, now())
    );
    let results = [a.unwrap(), b.unwrap()];
    assert_eq!(results.into_iter().filter(|value| *value).count(), 1);
    let before = fixture
        .db
        .collection::<Document>(OUTBOX)
        .find(doc! {})
        .sort(doc! {"sequence":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(fixture.count(EVIDENCE).await, 0);
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), config())
        .await
        .unwrap();
    assert!(!reopened.project_one(&id, now() + 1).await.unwrap());
    let after = fixture
        .db
        .collection::<Document>(OUTBOX)
        .find(doc! {})
        .sort(doc! {"sequence":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(before, after);
    let first = &before[0];
    let second = &before[1];
    let payload: serde_json::Value =
        serde_json::from_slice(second.get_binary_generic("payload").unwrap()).unwrap();
    assert_eq!(
        payload["previous_event_digest"].as_str(),
        first.get_str("digest").ok()
    );
    let events = fixture.wire_events().await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "stream_open");
    assert_eq!(events[1]["facts"], serde_json::json!([]));
    super::delivery_contract::recover_actual_events(&fixture).await;
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_binding_keeps_source_and_progress_unchanged() {
    let fixture = Fixture::new(true).await;
    let parent = fixture
        .parent_with_generation("legacy-unknown-generation", None)
        .await;
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), MINUTE)
        .await
        .unwrap();
    let capture = fixture.capture();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    assert!(!capture
        .project_one(&source_id(&parent, 2), now())
        .await
        .unwrap());
    let held = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find_one(doc! {"_id":source_id(&parent, 2)})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.get_str("projection_hold").unwrap(), "binding_missing");
    assert_eq!(fixture.count(CAPTURES).await, 2);
    assert_eq!(fixture.count(OUTBOX).await, 2);
    assert_eq!(fixture.count(EVIDENCE).await, 0);
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn bounded_long_projection_survives_source_deletion_restart_and_two_replicas() {
    let fixture = Fixture::new(true).await;
    let parent = fixture.parent("zero-balance").await;
    fixture
        .store
        .checkpoint_usage_one(parent.id.as_str(), "2026-09-05T02:00:07Z")
        .await
        .unwrap();
    let capture = fixture.capture();
    let stream_before = fixture
        .db
        .collection::<Document>(STREAMS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    capture
        .project_one(&source_id(&parent, 1), now())
        .await
        .unwrap();
    let id = source_id(&parent, 2);
    capture.project_one(&id, now()).await.unwrap();
    assert_eq!(fixture.count(EVIDENCE).await, 64);
    let incomplete = fixture
        .db
        .collection::<Document>(CAPTURES)
        .find_one(doc! {"_id":&id})
        .await
        .unwrap()
        .unwrap();
    assert!(!incomplete.get_bool("complete").unwrap());
    fixture
        .db
        .collection::<Document>(crate::store::sandbox_usage::EVENTS)
        .delete_many(doc! {})
        .await
        .unwrap();
    fixture
        .db
        .collection::<Document>("sandboxes")
        .delete_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap();
    let reopened = CaptureStore::open(&fixture.client, fixture.db.name(), config())
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        capture.project_one(&id, now()),
        reopened.project_one(&id, now())
    );
    left.unwrap();
    right.unwrap();
    assert!(!reopened.project_one(&id, now()).await.unwrap());
    let stream_after = fixture
        .db
        .collection::<Document>(STREAMS)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stream_before.get_str("_id").unwrap(),
        stream_after.get_str("_id").unwrap()
    );
    assert_eq!(
        stream_before
            .get_document("record")
            .unwrap()
            .get_str("source_epoch")
            .unwrap(),
        stream_after
            .get_document("record")
            .unwrap()
            .get_str("source_epoch")
            .unwrap()
    );
    let facts = fixture
        .db
        .collection::<Document>(EVIDENCE)
        .find(doc! {})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(facts.len(), 121);
    let mut total = layer2_core::sandbox_usage::UsageTotals::default();
    for row in facts {
        let fact: layer2_core::sandbox_usage_evidence::MinuteFact = records::decode(&row).unwrap();
        total = total.checked_add(&fact.quantity).unwrap();
    }
    assert_eq!(total.cpu_millis_seconds, 7_207_000);
    assert_eq!(total.memory_mib_seconds, 7_207 * 2_048);
    assert_eq!(total.uptime_seconds, 7_207);
    let rows = fixture
        .db
        .collection::<Document>(OUTBOX)
        .find(doc! {})
        .sort(doc! {"sequence":1})
        .await
        .unwrap()
        .try_collect::<Vec<_>>()
        .await
        .unwrap();
    assert_eq!(rows.len(), 4);
    let events = fixture.wire_events().await;
    assert_eq!(events.len(), 4);
    let facts: Vec<_> = events
        .iter()
        .filter_map(|event| event["facts"].as_array())
        .flatten()
        .collect();
    assert_eq!(facts.len(), 121);
    assert!(facts
        .iter()
        .all(|fact| fact["resource"]["home_id"].is_null()
            && fact["resource"]["home_generation"].is_null()
            && fact["resource"]["runtime_generation"] == "1"));
    assert_eq!(
        rows.iter()
            .map(|row| row.get_i64("part").unwrap())
            .collect::<Vec<_>>(),
        [-1, 0, 0, 1]
    );
    let mut previous: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let bytes = row.get_binary_generic("payload").unwrap();
        let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(serde_json_canonicalizer::to_vec(&value).unwrap(), *bytes);
        assert_eq!(value["sequence"], (index + 1).to_string());
        assert_eq!(value["previous_event_digest"].as_str(), previous.as_deref());
        assert_eq!(
            row.get_str("charge_disposition").unwrap(),
            "nonbillable_shadow"
        );
        previous = Some(records::hash(bytes));
        assert_eq!(row.get_str("digest").unwrap(), previous.as_deref().unwrap());
    }
    super::delivery_contract::recover_actual_events(&fixture).await;
    fixture.db.drop().await.unwrap();
}
