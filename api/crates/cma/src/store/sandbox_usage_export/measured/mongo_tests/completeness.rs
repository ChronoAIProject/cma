use super::*;
use crate::sandbox_usage_export::completeness_contract::{Basis, Statement};

async fn completeness_fixture() -> Fixture {
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
    f.bind().await;
    f
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_retained_repair_publishes_successor_outside_replay_horizon() {
    let f = completeness_fixture().await;
    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());
    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());

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
        .unwrap_or_else(|_| panic!("read measured completeness statement"))
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
        .unwrap_or_else(|_| panic!("read repaired completeness statement"))
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

async fn current_statement(f: &Fixture) -> Option<Statement> {
    f.store
        .retained_statement(
            &mut f.store.client.start_session().await.unwrap(),
            &f.stream.id,
            f.minute,
            Basis::Measured,
        )
        .await
        .unwrap_or_else(|_| panic!("read measured completeness statement"))
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_mixed_flag_takeover_keeps_measured_statement_current() {
    use super::refresh::{head, retry};
    use super::refresh_source::{Response, Source};

    let f = completeness_fixture().await;
    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());
    let mut config = f.store.config.clone();
    config.completeness_enabled = false;
    let disabled = CaptureStore::open(&f.store.client, f.db.name(), config)
        .await
        .unwrap();
    let source = Source::new(&f).await;

    assert!(source.run_with_store(&f, &disabled).await);
    let original = head(&f).await;
    let events = f.rows(OUTBOX).await;
    assert!(current_statement(&f).await.is_none());

    retry(&f).await;
    assert!(!source.run(&f).await);
    let first = current_statement(&f).await.unwrap();
    assert_eq!(first.revision.0, 1);
    assert!(first.producer_hold.is_none());

    retry(&f).await;
    source.set(Response::MemoryTimeout);
    assert!(!source.run_with_store(&f, &disabled).await);
    assert_retained_refresh_hold(&f, &original, &events).await;
    let held = current_statement(&f).await.unwrap();
    assert_eq!(held.revision.0, first.revision.0 + 1);
    assert_eq!(held.facts, first.facts);
    assert_eq!(
        held.producer_hold.as_deref(),
        Some("refresh_source_unavailable")
    );
    let delivery = f
        .store
        .claim_completeness(&held.statement_id, &"f".repeat(64))
        .await
        .unwrap()
        .unwrap();
    let statement =
        Statement::from_value(serde_json::from_slice(&delivery.payload).unwrap()).unwrap();
    assert_eq!(statement.producer_hold, held.producer_hold);

    retry(&f).await;
    source.set(Response::Values(60.0));
    assert!(!source.run_with_store(&f, &disabled).await);
    let recovered = current_statement(&f).await.unwrap();
    assert_eq!(recovered.revision.0, held.revision.0 + 1);
    assert!(recovered.producer_hold.is_none());
    assert_eq!(recovered.facts, first.facts);
    assert_eq!(head(&f).await, original);

    retry(&f).await;
    source.set(Response::Values(30.0));
    assert!(source.run_with_store(&f, &disabled).await);
    let corrected = current_statement(&f).await.unwrap();
    assert_eq!(corrected.revision.0, recovered.revision.0 + 1);
    assert_eq!(corrected.facts[0].event_id, head(&f).await.event_id);
    assert_eq!(corrected.facts[0].evidence_revision.0, 2);
    assert!(corrected.producer_hold.is_none());

    retry(&f).await;
    assert!(!source.run(&f).await);
    assert_eq!(
        current_statement(&f).await.unwrap().revision,
        corrected.revision
    );
    f.db.drop().await.unwrap();
}

async fn rebuild_due(f: &Fixture) {
    let updated = f
        .store
        .collection(INVENTORY)
        .update_one(
            doc! {
                "_id.usage_completeness_rebuild_v1": {"$exists":true},
                "source": &f.stream.id,
                "minute": f.minute as i64,
            },
            doc! {"$set":{"due_at":mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert_eq!(updated.matched_count, 1);
    f.store.rebuild_measured_completeness().await.unwrap();
}

async fn assert_retained_refresh_hold(f: &Fixture, original: &FactHead, outbox: &[Document]) {
    assert_eq!(super::refresh::head(f).await, *original);
    assert_eq!(f.rows(OUTBOX).await, outbox);
    let lease = f.lease().await;
    let hold = f
        .store
        .collection(INVENTORY)
        .find_one(doc! { "_id":keys::id(&keys::hold(&lease.stream.id, f.minute)) })
        .await
        .unwrap()
        .unwrap();
    f.store.release_measured(&lease).await.unwrap();
    assert_eq!(
        hold.get_str("reason").unwrap(),
        "refresh_source_unavailable"
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn completeness_pending_rebuild_respects_current_refresh_hold() {
    use super::refresh::{head, retry};
    use super::refresh_source::{Response, Source};
    use crate::store::sandbox_usage_export::completeness::MinuteOutcome;

    let f = completeness_fixture().await;
    let source = Source::new(&f).await;
    assert!(source.run(&f).await);
    let original = head(&f).await;
    let repair = super::super::repair::MeasuredRepair {
        source_stream: f.stream.id.clone(),
        minute: f.minute,
        evidence_digest: original.evidence_id.clone(),
        predecessor_event: original.event_id.clone(),
    };
    // The production repair CLI opens the store with emission disabled.
    let mut repair_config = f.store.config.clone();
    repair_config.completeness_enabled = false;
    let repair_store = CaptureStore::open(&f.store.client, f.db.name(), repair_config)
        .await
        .unwrap();
    assert!(current_statement(&f).await.is_none());

    retry(&f).await;
    source.set(Response::TotalTimeout);
    assert!(!source.run(&f).await);
    assert_eq!(head(&f).await, original);

    assert!(f.store.backfill_completeness(&f.stream.id).await.unwrap());
    rebuild_due(&f).await;
    assert!(current_statement(&f).await.is_none());
    assert_eq!(head(&f).await, original);
    let retained = f.rows(OUTBOX).await;
    assert!(!repair_store.repair_measured(&repair, true).await.unwrap());
    assert!(!f.store.repair_measured(&repair, true).await.unwrap());
    assert!(current_statement(&f).await.is_none());
    assert_retained_refresh_hold(&f, &original, &retained).await;

    // A fresh complete refresh can clear the authoritative hold without
    // changing the retained fact head.
    retry(&f).await;
    source.set(Response::Values(60.0));
    assert!(!source.run(&f).await);
    let first = current_statement(&f).await.unwrap();
    assert!(first.producer_hold.is_none());
    assert_eq!(head(&f).await, original);

    // Model a queued rebuild for a retained fact which already has a
    // published statement. A same-mapping-revision refresh can still hold it.
    let lease = f.lease().await;
    f.store
        .record_measured_completeness(
            &mut f.store.client.start_session().await.unwrap(),
            &lease.stream,
            f.minute,
            &original,
            MinuteOutcome::Retry("allocated_statement_pending"),
        )
        .await
        .unwrap_or_else(|_| panic!("queue retained completeness rebuild"));
    f.store.release_measured(&lease).await.unwrap();

    retry(&f).await;
    source.set(Response::MemoryTimeout);
    assert!(!source.run(&f).await);
    let held = current_statement(&f).await.unwrap();
    assert_eq!(held.revision.0, first.revision.0 + 1);
    assert_eq!(
        held.measured_mapping_revision,
        first.measured_mapping_revision
    );
    assert_eq!(
        held.producer_hold.as_deref(),
        Some("refresh_source_unavailable")
    );

    assert!(!repair_store.repair_measured(&repair, true).await.unwrap());
    assert!(!f.store.repair_measured(&repair, true).await.unwrap());
    assert_retained_refresh_hold(&f, &original, &retained).await;
    rebuild_due(&f).await;
    let after_rebuild = current_statement(&f).await.unwrap();
    assert_eq!(after_rebuild.revision, held.revision);
    assert_eq!(after_rebuild.producer_hold, held.producer_hold);
    assert_eq!(head(&f).await, original);

    retry(&f).await;
    source.set(Response::Values(60.0));
    assert!(!source.run(&f).await);
    let recovered = current_statement(&f).await.unwrap();
    assert_eq!(recovered.revision.0, held.revision.0 + 1);
    assert!(recovered.producer_hold.is_none());
    assert_eq!(head(&f).await, original);
    assert!(repair_store.repair_measured(&repair, true).await.unwrap());
    assert!(repair_store.repair_measured(&repair, true).await.unwrap());
    assert_eq!(
        current_statement(&f).await.unwrap().revision,
        recovered.revision
    );
    assert_eq!(head(&f).await, original);
    assert_eq!(
        f.store
            .collection(INVENTORY)
            .count_documents(doc! {"_id.usage_completeness_rebuild_v1":{"$exists":true}})
            .await
            .unwrap(),
        0,
    );
    f.db.drop().await.unwrap();
}
