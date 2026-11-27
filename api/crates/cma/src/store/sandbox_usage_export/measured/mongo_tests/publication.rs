use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn missing_all_owner_minute_is_null_and_reopen_keeps_frozen_bytes() {
    let f = Fixture::new().await;
    let lease = f.lease().await;
    let input = f.input(&lease).await;
    assert!(input.bindings.is_empty());
    assert!(input.hold.is_none());
    let publication = input.freeze(vec![], true).unwrap();
    assert!(f
        .store
        .publish_measured(&lease, &publication)
        .await
        .unwrap());
    let events = f.events().await;
    assert_eq!(events.len(), 2);
    let fact = &events[1]["facts"][0];
    assert!(fact["quantities"]["sandbox_cpu_core_nanoseconds_v1"].is_null());
    assert_eq!(fact["coverage"]["disk"]["quality"], "missing");
    let frozen = f.rows(OUTBOX).await;
    assert!(f
        .store
        .publish_measured(&lease, &publication)
        .await
        .is_err());
    assert_eq!(f.rows(OUTBOX).await, frozen);
    f.store.release_measured(&lease).await.unwrap();
    let reopened = CaptureStore::open(&f.store.client, f.db.name(), f.store.config.clone())
        .await
        .unwrap();
    let lease = reopened
        .claim_measured(&lease.stream.key())
        .await
        .unwrap()
        .unwrap();
    let publication = reopened
        .prepare_measured(&lease)
        .await
        .unwrap()
        .unwrap()
        .freeze(vec![], true)
        .unwrap();
    assert!(!reopened
        .publish_measured(&lease, &publication)
        .await
        .unwrap());
    assert_eq!(f.rows(OUTBOX).await, frozen);
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn two_corrections_cas_and_retained_repair_survive_deleted_source_rows() {
    let f = Fixture::new().await;
    f.bind().await;
    let first = f.lease().await;
    let original = f.input(&first).await.freeze(f.samples(0.0), true).unwrap();
    f.store.publish_measured(&first, &original).await.unwrap();
    let original_bytes = f.rows(OUTBOX).await;
    f.store.release_measured(&first).await.unwrap();
    let second = f.lease().await;
    let input = f.input(&second).await;
    let a = input.clone().freeze(f.samples(30.0), true).unwrap();
    let b = input.freeze(f.samples(60.0), true).unwrap();
    let (one, two) = tokio::join!(
        f.store.publish_measured(&second, &a),
        f.store.publish_measured(&second, &b)
    );
    assert_eq!(
        [one, two].iter().filter(|r| matches!(r, Ok(true))).count(),
        1
    );
    let events = f.events().await;
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["kind"], "correction");
    assert_eq!(
        events[2]["correction"]["expected_predecessor_event_id"],
        events[1]["event_id"]
    );
    assert_eq!(
        events[2]["correction"]["replacement"]["fact_id"],
        events[1]["facts"][0]["fact_id"]
    );
    let retained = f.rows(OUTBOX).await;
    for row in &original_bytes {
        assert!(retained.contains(row));
    }
    f.store.release_measured(&second).await.unwrap();
    for name in [
        sandbox_metric_bindings::COLLECTION,
        usage_reports::CONTROL,
        CAPTURES,
        STREAMS,
    ] {
        f.store.collection(name).delete_many(doc! {}).await.unwrap();
    }
    let request = super::super::repair::MeasuredRepair {
        source_stream: f.stream.id.clone(),
        minute: f.minute,
        evidence_digest: original.digest.clone(),
        predecessor_event: events[2]["event_id"].as_str().unwrap().into(),
    };
    assert!(!f.store.repair_measured(&request, false).await.unwrap());
    assert!(f.store.repair_measured(&request, true).await.unwrap());
    let after = f.rows(OUTBOX).await;
    assert_eq!(after.len(), 4);
    assert!(f.store.repair_measured(&request, true).await.unwrap());
    assert_eq!(f.rows(OUTBOX).await, after);
    let events = f.events().await;
    assert_eq!(
        events[3]["correction"]["replacement"]["quantities"]["sandbox_cpu_core_nanoseconds_v1"],
        "0"
    );
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cross_owner_physical_claims_retain_hold_without_an_invented_fact() {
    let f = Fixture::new().await;
    let mut other = f.bind().await;
    other.id = "foreign-binding".into();
    other.sandbox_id = "foreign-sandbox".into();
    other.owner_subject = "foreign-owner".into();
    f.store
        .collection(sandbox_metric_bindings::COLLECTION)
        .insert_one(binding_document(&other))
        .await
        .unwrap();
    let lease = f.lease().await;
    let input = f.input(&lease).await;
    assert_eq!(input.bindings.len(), 2);
    let mut samples = f.samples(60.0);
    samples[0].samples[0].value = Some(-0.0);
    let publication = input.clone().freeze(samples.clone(), true).unwrap();
    assert_eq!(
        publication.manifest.hold.as_deref(),
        Some("physical_attribution_conflict")
    );
    let restored: super::super::freeze::Manifest =
        serde_json::from_slice(&publication.bytes).unwrap();
    assert_eq!(restored.rejected_samples, samples);
    assert_eq!(restored.rejected_sample_bits[0][0][1], "8000000000000000");
    assert_eq!(serde_json::to_vec(&restored).unwrap(), publication.bytes);

    samples[0].samples[0].value = None;
    assert!(matches!(
        input.freeze(samples, true),
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert!(!f
        .store
        .publish_measured(&lease, &publication)
        .await
        .unwrap());
    assert!(f.rows(OUTBOX).await.is_empty());
    assert_eq!(f.rows(EVIDENCE).await.len(), 1);
    assert!(f
        .rows(INVENTORY)
        .await
        .iter()
        .any(|r| r.get_str("reason").ok() == Some("physical_attribution_conflict")));
    f.db.drop().await.unwrap();
}
