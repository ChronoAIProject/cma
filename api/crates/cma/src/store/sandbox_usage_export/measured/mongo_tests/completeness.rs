use super::*;
use crate::sandbox_usage_export::completeness_contract::{Basis, Statement};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_retained_repair_publishes_successor_outside_replay_horizon() {
    let mut f = Fixture::new().await;
    let mut config = f.store.config.clone();
    config.completeness_enabled = true;
    f.store = CaptureStore::open(&f.store.client, f.db.name(), config)
        .await
        .unwrap();
    let source_row = f
        .store
        .collection(CAPTURES)
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    let mut first: Captured = decode(&source_row).unwrap();
    first.source.revision = 1;
    first.source.source_seq = 1;
    first.source.kind = UsageEventKind::Created;
    first.source.ended_at = first.source.started_at;
    first.source.quantity = first.source.allocation.usage(0).unwrap();
    first.previous_revision = None;
    f.store.collection(CAPTURES).insert_one(doc! {"_id":first.source.transaction_id(),"document_schema_version":1_i64,
        "stream_id":&first.stream_id,"sandbox_id":first.source.sandbox_id.as_str(),"owner":first.source.owner.as_str(),
        "revision":1_i64,"source_digest":hash(&serde_json::to_vec(&first).unwrap()),"record":encode(&first).unwrap(),
        "complete":false,"next_second":first.source.started_at as i64}).await.unwrap();
    f.store
        .project_one(&first.source.transaction_id(), usage_reports::now())
        .await
        .unwrap();
    f.store
        .project_one(source_row.get_str("_id").unwrap(), usage_reports::now())
        .await
        .unwrap();
    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());
    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());

    f.bind().await;
    let first = f.lease().await;
    let original = f.input(&first).await.freeze(f.samples(0.0), true).unwrap();
    f.store.publish_measured(&first, &original).await.unwrap();
    f.store.release_measured(&first).await.unwrap();
    let second = f.lease().await;
    let correction = f
        .input(&second)
        .await
        .freeze(f.samples(60.0), true)
        .unwrap();
    f.store
        .publish_measured(&second, &correction)
        .await
        .unwrap();
    f.store.release_measured(&second).await.unwrap();
    let current = f
        .store
        .retained_statement(
            &mut f.store.client.start_session().await.unwrap(),
            &f.stream.id,
            f.minute,
            Basis::Measured,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision.0, 2);
    let mut simulated = f.lease().await;
    simulated.now = f.minute + 31 * 86400;
    simulated.stream.next_minute = simulated.stream.capture_start + 60;
    simulated.stream.replay_minute = simulated.stream.capture_start;
    simulated.stream.replay_turn = true;
    let input = f.store.prepare_measured(&simulated).await.unwrap().unwrap();
    assert!(!input.replay && input.minute != f.minute);
    f.store.release_measured(&simulated).await.unwrap();
    let old = current.facts[0].event_id.clone();
    // The explicitly invoked repair must preserve the previously enabled
    // completeness protocol, even when its own local emission flag is off.
    let mut config = f.store.config.clone();
    config.completeness_enabled = false;
    let repair_store = CaptureStore::open(&f.store.client, f.db.name(), config)
        .await
        .unwrap();
    let request = super::super::repair::MeasuredRepair {
        source_stream: f.stream.id.clone(),
        minute: f.minute,
        evidence_digest: original.digest.clone(),
        predecessor_event: old,
    };
    assert!(repair_store.repair_measured(&request, true).await.unwrap());
    let mut session = f.store.client.start_session().await.unwrap();
    let repaired = f
        .store
        .retained_statement(&mut session, &f.stream.id, f.minute, Basis::Measured)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repaired.revision.0, 3);
    assert_ne!(repaired.facts, current.facts);
    assert_eq!(repaired.facts[0].evidence_revision.0, 3);
    assert!(repair_store.repair_measured(&request, true).await.unwrap());
    let mut observed = Vec::new();
    for id in f.store.completeness_delivery_page("").await.unwrap() {
        let attempt = f
            .store
            .claim_completeness(&id, &"f".repeat(64))
            .await
            .unwrap()
            .unwrap();
        let stmt =
            Statement::from_value(serde_json::from_slice(&attempt.payload).unwrap()).unwrap();
        if stmt.basis == Basis::Measured && stmt.minute_start_ms.0 == f.minute * 1000 {
            observed.push(stmt.revision.0);
        }
        let receipt = crate::store::sandbox_usage_export::CompletenessReceipt {
            statement_id: attempt.id.clone(),
            receipt_id: uuid::Uuid::new_v4().to_string(),
            statement_digest: attempt.digest.clone(),
            accepted_at_ms: (usage_reports::now() * 1000).to_string(),
            custody: "persisted".into(),
        };
        f.store
            .finish_completeness(
                &attempt,
                crate::store::sandbox_usage_export::CompletenessResult::Accepted(receipt),
            )
            .await
            .unwrap();
    }
    observed.sort_unstable();
    assert_eq!(observed, vec![1, 2, 3]);
    f.db.drop().await.unwrap();
}
