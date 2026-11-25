use super::{model::*, IndexedCustody, MANIFESTS, PROJECTIONS};
use crate::store::measured_capture::{
    ledger::{FreezeRequest, MeasuredLedger, ReadRequest},
    preview::custody_samples,
    tests::fixture_binding,
    Capture, MeasuredCapture, MeasuredObservation,
};
use crate::store::sandbox_usage;
use layer2_core::{
    sandbox_metrics::{
        measured_evidence::{
            batch::{EvidenceScope, FrozenEvidence},
            capture_observations,
        },
        Binding, Range,
    },
    sandbox_usage::{Allocation, SandboxUsage, UsageEvent, UsageEventKind},
    SandboxId, SubjectId,
};
use layer3_adapter::sandbox_metrics::{decode_raw_export, Selection, SourceMetric};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};

#[path = "sharded_mongo_tests.rs"]
mod sharded_mongo_tests;

fn observation(binding: &Binding, timestamp: u64, value: f64) -> MeasuredObservation {
    let selection = Selection {
        cluster: binding.cluster.clone(),
        namespace: binding.namespace.clone(),
        metric: SourceMetric::PvcUsed,
        names: vec![binding.pvc_name.clone()],
        from: binding.valid_from,
        to: binding.observed_at,
    };
    let raw = serde_json::json!({
        "metric": {
            "cluster": binding.cluster, "namespace": binding.namespace,
            "__name__": selection.metric.name(), "job": "kubelet",
            "metrics_path": selection.metric.metrics_path(), "persistentvolumeclaim": binding.pvc_name,
            "pvc_uid": binding.pvc_uid,
        },
        "timestamps": [timestamp], "values": [value],
    });
    let source = decode_raw_export(raw.to_string().as_bytes(), &selection).unwrap();
    capture_observations(
        binding,
        1,
        selection.metric,
        &source,
        Range {
            from: selection.from,
            to: selection.to,
        },
    )
    .unwrap()
    .remove(0)
}

fn scope(label: &str, from_ms: u64, to_ms: u64) -> EvidenceScope {
    EvidenceScope {
        owner_id: "owner-a".into(),
        sandbox_id: "sbx_synthetic".into(),
        window_id: label.into(),
        from_ms,
        to_ms,
        rate_card_id: "fixture".into(),
        rate_card_version: 1,
    }
}

async fn allocation(database: &Database) -> UsageEvent {
    let allocation = Allocation {
        running: false,
        cpu_millis: 0,
        memory_mib: 0,
        disk_gib: 1,
    };
    let quantity = allocation.usage(300).unwrap();
    let event = UsageEvent {
        sandbox_id: SandboxId("sbx_synthetic".into()),
        owner: SubjectId::new("owner-a"),
        revision: 1,
        source_seq: 1,
        kind: UsageEventKind::Baseline,
        started_at: 3500,
        ended_at: 3800,
        allocation: allocation.clone(),
        next_allocation: allocation.clone(),
        status: "paused".into(),
        quantity: quantity.clone(),
    };
    let state = SandboxUsage {
        sandbox_id: event.sandbox_id.clone(),
        owner: event.owner.clone(),
        name: "fixture".into(),
        status: "paused".into(),
        allocation,
        created_at: 3500,
        tracking_started_at: 3500,
        last_observed_at: 3800,
        source_seq: 1,
        source_revision: 1,
        revision: 1,
        has_started: true,
        complete_lifecycle: true,
        totals: quantity,
    };
    database
        .collection::<Document>(sandbox_usage::STATES)
        .insert_one(sandbox_usage::state_document(&state).unwrap())
        .await
        .unwrap();
    database
        .collection::<Document>(sandbox_usage::EVENTS)
        .insert_one(sandbox_usage::event_document(&event).unwrap())
        .await
        .unwrap();
    event
}

async fn compare(
    database: &Database,
    indexed: &IndexedCustody,
    capture: &MeasuredCapture,
    scope: &EvidenceScope,
    event: &UsageEvent,
    previous: Option<&FrozenEvidence>,
) -> FrozenEvidence {
    let mut session = database
        .client()
        .start_session()
        .snapshot(true)
        .await
        .unwrap();
    let exhaustive = capture.read_positions(&mut session, 100).await.unwrap();
    let projected = indexed
        .read_scope(scope, 3_800_000, &mut session)
        .await
        .unwrap();
    let a = custody_samples(&exhaustive).unwrap();
    let b = custody_samples(&projected).unwrap();
    let freeze = |samples| {
        FrozenEvidence::freeze(
            scope.clone(),
            3_800_000,
            samples,
            std::slice::from_ref(event),
            previous,
        )
        .unwrap()
    };
    let expected = freeze(&a);
    assert_eq!(freeze(&b), expected);
    expected
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn indexed_custody_matches_exhaustive_and_refuses_missing_projection_before_freeze() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let left = Client::with_uri_str(&uri).await.unwrap();
    let right = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_index_{}", uuid::Uuid::new_v4().simple());
    let database = left.database(&name);
    let capture = MeasuredCapture::new(&database);
    let indexed = IndexedCustody::new(&database, "a".repeat(64)).unwrap();
    let peer_index = IndexedCustody::new(&right.database(&name), "a".repeat(64)).unwrap();
    database
        .create_collection(crate::store::migrations::measured_custody::COLLECTION)
        .await
        .unwrap();
    database.create_collection(PROJECTIONS).await.unwrap();
    database.create_collection(MANIFESTS).await.unwrap();
    let mut a = fixture_binding("owner-a");
    a.valid_from = 3500;
    a.observed_at = 3800;
    let mut middle = a.clone();
    middle.pvc_uid = "pvc-transfer".into();
    middle.id = "binding-transfer".into();
    let mut foreign = middle.clone();
    foreign.owner_subject = "owner-b".into();
    foreign.sandbox_id = "sbx_other".into();
    foreign.id = "binding-foreign".into();
    let mut singleton = a.clone();
    singleton.pvc_uid = "pvc-singleton".into();
    singleton.id = "binding-singleton".into();
    let first = observation(&a, 3_590_000, 20.0);
    let last = observation(&a, 3_610_000, 30.0);
    for sample in [
        &first,
        &last,
        &observation(&middle, 3_660_000, 40.0),
        &observation(&foreign, 3_670_000, 50.0),
        &observation(&middle, 3_680_000, 60.0),
        &observation(&singleton, 3_670_000, 100.0),
    ] {
        assert_eq!(capture.capture(sample).await.unwrap(), Capture::Inserted);
    }
    let event = allocation(&database).await;
    for (_, index) in crate::store::migrations::measured_batches::inspect(&database)
        .await
        .unwrap()
    {
        crate::store::migrations::measured_batches::create(&database, index)
            .await
            .unwrap();
    }
    let legacy = MeasuredLedger::new(&database);
    let old_scope = scope("legacy", 3_560_000, 3_570_000);
    let old_request = FreezeRequest {
        scope: old_scope.clone(),
        source_cutoff_ms: 3_800_000,
        correction_of: None,
    };
    let old = legacy.freeze(old_request.clone()).await.unwrap();
    indexed.begin().await.unwrap();
    let active = MeasuredCapture::new(&right.database(&name)).with_indexed(peer_index.clone());
    assert!(active
        .capture(&observation(&a, 3_620_000, 40.0))
        .await
        .is_err());
    assert_eq!(capture.rows.count_documents(doc! {}).await.unwrap(), 6);
    assert!(indexed.certify().await.is_err());
    assert_eq!(indexed.advance().await.unwrap(), (6, false));
    assert_eq!(peer_index.advance().await.unwrap(), (6, true));
    let proof = indexed.certify().await.unwrap();
    assert_eq!(proof.original_rows, 6);
    assert_eq!(peer_index.certify().await.unwrap(), proof);
    let window = scope("straddling", 3_595_000, 3_605_000);
    let expected = compare(&database, &indexed, &capture, &window, &event, None).await;
    compare(
        &database,
        &indexed,
        &capture,
        &scope("transfer", 3_658_000, 3_682_000),
        &event,
        None,
    )
    .await;
    compare(
        &database,
        &indexed,
        &capture,
        &scope("overlap", 3_600_000, 3_615_000),
        &event,
        None,
    )
    .await;
    let ledger = MeasuredLedger::new(&database).with_indexed(indexed.clone());
    for (collection, index) in crate::store::migrations::measured_index::inspect(&database)
        .await
        .unwrap()
    {
        crate::store::migrations::measured_index::create(&database, collection, index)
            .await
            .unwrap();
    }
    let initial = FreezeRequest {
        scope: window.clone(),
        source_cutoff_ms: 3_800_000,
        correction_of: None,
    };
    assert_eq!(ledger.freeze(initial.clone()).await.unwrap(), expected);

    // Late competing attribution against an older point changes the correction.
    let mut competing = a.clone();
    competing.owner_subject = "owner-b".into();
    competing.sandbox_id = "sbx_other".into();
    let late = observation(&competing, 3_590_000, 20.0);
    assert_eq!(active.capture(&late).await.unwrap(), Capture::Disputed);
    assert_eq!(active.capture(&late).await.unwrap(), Capture::Disputed);
    let correction = compare(
        &database,
        &indexed,
        &capture,
        &window,
        &event,
        Some(&expected),
    )
    .await;
    let competing_initial = compare(&database, &indexed, &capture, &window, &event, None).await;
    assert_ne!(competing_initial.reference(), expected.reference());
    assert!(ledger
        .publish_interval(competing_initial, usize::MAX)
        .await
        .is_err());
    assert_eq!(ledger.freeze(initial.clone()).await.unwrap(), expected);

    assert_eq!(
        ledger
            .freeze(FreezeRequest {
                correction_of: Some(expected.reference()),
                ..initial.clone()
            })
            .await
            .unwrap(),
        correction
    );
    assert_eq!(
        ledger
            .read(ReadRequest {
                scope: window.clone(),
                revision: 1
            })
            .await
            .unwrap(),
        expected
    );

    assert_eq!(
        active
            .capture(&observation(&a, 3_600_000, 25.0))
            .await
            .unwrap(),
        Capture::Inserted
    );
    let competing_correction = compare(
        &database,
        &indexed,
        &capture,
        &window,
        &event,
        Some(&expected),
    )
    .await;
    assert_ne!(competing_correction.reference(), correction.reference());
    assert!(ledger
        .publish_interval(competing_correction, usize::MAX)
        .await
        .is_err());
    assert_eq!(
        ledger
            .freeze(FreezeRequest {
                correction_of: Some(expected.reference()),
                ..initial.clone()
            })
            .await
            .unwrap(),
        correction
    );

    let peer_ledger = MeasuredLedger::new(&right.database(&name)).with_indexed(peer_index.clone());
    assert_eq!(ledger.freeze(old_request).await.unwrap(), old);
    assert!(ledger
        .freeze(FreezeRequest {
            scope: scope("overlap-legacy", 3_565_000, 3_575_000),
            ..initial.clone()
        })
        .await
        .is_err());
    let left_claim = FreezeRequest {
        scope: scope("left", 3_620_000, 3_635_000),
        ..initial.clone()
    };
    let right_claim = FreezeRequest {
        scope: scope("right", 3_630_000, 3_645_000),
        ..initial.clone()
    };
    let (left_claim, right_claim) =
        tokio::join!(ledger.freeze(left_claim), peer_ledger.freeze(right_claim));
    assert_ne!(left_claim.is_ok(), right_claim.is_ok());
    let winner = left_claim.or(right_claim).unwrap();
    assert!(ledger
        .freeze(FreezeRequest {
            scope: scope(
                "adjacent",
                winner.scope().to_ms,
                winner.scope().to_ms + 1000
            ),
            ..initial.clone()
        })
        .await
        .is_ok());

    // Move the indexed search key while keeping the canonical original and
    // authenticated time directory intact. The omitted leaf must be detected.
    let source = database
        .collection::<Document>(crate::store::measured_capture::COLLECTION)
        .find_one(
            doc! {"_id": crate::store::measured_capture::sample_id(first.physical_sample_id())},
        )
        .await
        .unwrap()
        .unwrap();
    let projected = Projection::from_source(1, &source).unwrap();
    let rows = database.collection::<Document>(PROJECTIONS);
    rows.update_one(
        doc! {"_id": projected.id()},
        doc! {"$set": {"hour": 99_i64}},
    )
    .await
    .unwrap();
    let new_window = scope("new", 3_605_000, 3_609_000);
    assert!(ledger
        .freeze(FreezeRequest {
            scope: new_window,
            ..initial.clone()
        })
        .await
        .is_err());
    assert_eq!(
        database
            .collection::<Document>(crate::store::migrations::measured_index::BATCHES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        4
    );
    rows.replace_one(doc! {"_id": projected.id()}, projected.document().unwrap())
        .await
        .unwrap();
    let manifest = database.collection::<Document>(MANIFESTS);
    let scope_key = scope_id("owner-a", "sbx_synthetic").unwrap();
    let root = {
        let mut session = database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .unwrap();
        indexed
            .root(&mut session, true)
            .await
            .map_err(super::store_error)
            .unwrap()
    };
    let hour = projected.hour();
    let id = hour_id(root.generation, hour);
    let current = manifest.find_one(doc! {"_id":&id}).await.unwrap().unwrap();
    let mut hour_directory: HourDirectory = open(&current, &id, root.generation, None).unwrap();
    hour_directory.scopes.remove(&scope_key);
    manifest
        .replace_one(
            doc! {"_id":&id},
            sealed(&id, root.generation, &hour_directory).unwrap(),
        )
        .await
        .unwrap();
    assert!(ledger
        .freeze(FreezeRequest {
            scope: scope("missing-scope", 3_605_000, 3_609_000),
            ..initial.clone()
        })
        .await
        .is_err());
    // Exact replay of already finalized evidence is independent of new discovery.
    assert_eq!(
        ledger
            .read(ReadRequest {
                scope: window,
                revision: 2
            })
            .await
            .unwrap(),
        correction
    );
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn concurrent_indexed_captures_converge_under_generation_root_contention() {
    const WRITERS: usize = 16;
    const SAMPLES: usize = 4;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_contention_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    let capture = MeasuredCapture::new(&database);
    let indexed = IndexedCustody::new(&database, "a".repeat(64)).unwrap();
    for collection in [
        crate::store::migrations::measured_custody::COLLECTION,
        PROJECTIONS,
        MANIFESTS,
    ] {
        database.create_collection(collection).await.unwrap();
    }
    indexed.begin().await.unwrap();
    assert_eq!(indexed.advance().await.unwrap(), (0, true));
    assert_eq!(indexed.certify().await.unwrap().original_rows, 0);
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(WRITERS));
    let mut tasks = Vec::new();
    for worker in 0..WRITERS {
        let peer = Client::with_uri_str(&uri).await.unwrap();
        let peer_db = peer.database(&name);
        let active = MeasuredCapture::new(&peer_db)
            .with_indexed(IndexedCustody::new(&peer_db, "a".repeat(64)).unwrap());
        let mut binding = fixture_binding(&format!("owner-{worker}"));
        binding.id = format!("binding-{worker}");
        binding.sandbox_id = format!("sbx_fixture_{worker}");
        binding.pvc_uid = format!("pvc-{worker}");
        binding.valid_from = 3500;
        binding.observed_at = 3800;
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            for offset in 0..SAMPLES {
                let sample = observation(&binding, 3_600_000 + offset as u64 * 1000, 20.0);
                assert_eq!(active.capture(&sample).await.unwrap(), Capture::Inserted);
            }
            let last = observation(&binding, 3_600_000 + (SAMPLES - 1) as u64 * 1000, 20.0);
            assert_eq!(active.capture(&last).await.unwrap(), Capture::Replay);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let expected = (WRITERS * SAMPLES) as u64;
    assert_eq!(
        capture.rows.count_documents(doc! {}).await.unwrap(),
        expected
    );
    assert_eq!(
        indexed.projections.count_documents(doc! {}).await.unwrap(),
        expected
    );
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    let root = indexed
        .root(&mut snapshot, true)
        .await
        .unwrap_or_else(|_| panic!("generation root missing"));
    assert_eq!(root.rows, expected);
    drop(snapshot);
    database.drop().await.unwrap();
}
