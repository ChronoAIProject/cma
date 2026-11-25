use super::*;
use crate::store::measured_capture::tests::observation_at;
use layer2_core::{
    sandbox_usage::{Allocation, SandboxUsage, UsageEventKind},
    SandboxId, SubjectId,
};
use mongodb::Client;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn measured_batches_are_immutable_and_corrections_are_idempotent_across_clients() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let left = Client::with_uri_str(&uri).await.unwrap();
    let right = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_frozen_{}", uuid::Uuid::new_v4().simple());
    let database = left.database(&name);
    let capture = MeasuredCapture::new(&database);
    database
        .create_collection(super::super::COLLECTION)
        .await
        .unwrap();
    let a = MeasuredLedger::new(&database);
    let b = MeasuredLedger::new(&right.database(&name));
    for (_, index) in measured_batches::inspect(&database).await.unwrap() {
        measured_batches::create(&database, index).await.unwrap();
    }
    assert!(measured_batches::inspect(&database)
        .await
        .unwrap()
        .is_empty());
    let sandbox_id = SandboxId("sbx_synthetic".into());
    let owner = SubjectId::new("owner-a");
    let allocation = Allocation {
        running: false,
        cpu_millis: 0,
        memory_mib: 0,
        disk_gib: 1,
    };
    let totals = allocation.usage(20).unwrap();
    let event = UsageEvent {
        sandbox_id: sandbox_id.clone(),
        owner: owner.clone(),
        revision: 1,
        source_seq: 1,
        kind: UsageEventKind::Baseline,
        started_at: 20,
        ended_at: 40,
        allocation: allocation.clone(),
        next_allocation: allocation.clone(),
        status: "paused".into(),
        quantity: totals.clone(),
    };
    let state = SandboxUsage {
        sandbox_id: sandbox_id.clone(),
        owner: owner.clone(),
        name: "synthetic".into(),
        status: "paused".into(),
        allocation,
        created_at: 20,
        tracking_started_at: 20,
        last_observed_at: 40,
        source_seq: 1,
        source_revision: 1,
        revision: 1,
        has_started: true,
        complete_lifecycle: true,
        totals,
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
    let scope = EvidenceScope {
        owner_id: owner.to_string(),
        sandbox_id: sandbox_id.to_string(),
        window_id: "window-fixture".into(),
        from_ms: 21_000,
        to_ms: 39_000,
        rate_card_id: "rate-fixture".into(),
        rate_card_version: 1,
    };
    let request = FreezeRequest {
        scope: scope.clone(),
        source_cutoff_ms: 40_000,
        correction_of: None,
    };
    let (first, peer) = tokio::join!(a.freeze(request.clone()), b.freeze(request.clone()));
    let first = first.unwrap();
    assert_eq!(first, peer.unwrap());
    let relabeled = FreezeRequest {
        scope: EvidenceScope {
            window_id: "another-label".into(),
            ..scope.clone()
        },
        ..request.clone()
    };
    assert!(b.freeze(relabeled).await.is_err());
    let overlapping = FreezeRequest {
        scope: EvidenceScope {
            window_id: "overlapping-window".into(),
            from_ms: 38_000,
            to_ms: 40_000,
            ..scope.clone()
        },
        ..request.clone()
    };
    assert!(b.freeze(overlapping).await.is_err());
    let original = a
        .rows
        .find_one(doc! {"_id":first.evidence_id()})
        .await
        .unwrap()
        .unwrap();
    capture
        .capture(&observation_at("owner-a", 1, 21_000, 100.0))
        .await
        .unwrap();
    capture
        .capture(&observation_at("owner-a", 1, 39_000, 200.0))
        .await
        .unwrap();
    assert_eq!(b.freeze(request.clone()).await.unwrap(), first);
    let correction = FreezeRequest {
        correction_of: Some(first.reference()),
        ..request.clone()
    };
    let first_bytes = original.get_str("payload").unwrap().len();
    assert!(matches!(
        b.freeze_with_history_limit(correction.clone(), first_bytes)
            .await,
        Err(CaptureError::Unavailable)
    ));
    assert_eq!(a.rows.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        b.read_exact_period(ExactPeriodReadRequest {
            period: PeriodReadRequest {
                owner_id: scope.owner_id.clone(),
                sandbox_id: scope.sandbox_id.clone(),
                from_ms: scope.from_ms,
                to_ms: scope.to_ms,
                source_cutoff_ms: 40_000,
                windows: vec![scope.clone()],
            },
            selected: vec![SelectedWindow {
                revision: first.revision(),
                reference: first.reference(),
            }],
        })
        .await
        .unwrap(),
        vec![first.clone()]
    );
    let (second, peer) = tokio::join!(a.freeze(correction.clone()), b.freeze(correction));
    let second = second.unwrap();
    assert_eq!(second, peer.unwrap());
    second.validate_successor(&first).unwrap();
    assert_eq!(
        a.rows
            .find_one(doc! {"_id":first.evidence_id()})
            .await
            .unwrap(),
        Some(original)
    );
    assert_eq!(a.rows.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        b.read(ReadRequest {
            scope: scope.clone(),
            revision: 1
        })
        .await
        .unwrap(),
        first
    );
    assert_eq!(
        b.read(ReadRequest {
            scope: scope.clone(),
            revision: 2
        })
        .await
        .unwrap(),
        second
    );
    let period = PeriodReadRequest {
        owner_id: scope.owner_id.clone(),
        sandbox_id: scope.sandbox_id.clone(),
        from_ms: scope.from_ms,
        to_ms: scope.to_ms,
        source_cutoff_ms: 40_000,
        windows: vec![scope.clone()],
    };
    assert_eq!(
        b.read_period(period.clone()).await.unwrap(),
        vec![second.clone()]
    );
    let replay = ExactPeriodReadRequest {
        period: period.clone(),
        selected: vec![SelectedWindow {
            revision: first.revision(),
            reference: first.reference(),
        }],
    };
    assert_eq!(
        b.read_exact_period(replay.clone()).await.unwrap(),
        vec![first.clone()]
    );
    let mut changed = replay;
    changed.selected[0].reference.evidence_sha256 = "a".repeat(64);
    assert!(b.read_exact_period(changed).await.is_err());
    assert!(b
        .read_period(PeriodReadRequest {
            from_ms: period.from_ms - 1000,
            ..period.clone()
        })
        .await
        .is_err());
    assert!(b
        .read_period(PeriodReadRequest {
            owner_id: "owner-b".into(),
            ..period.clone()
        })
        .await
        .is_err());
    assert!(b
        .read_period(PeriodReadRequest {
            source_cutoff_ms: 39_000,
            ..period
        })
        .await
        .is_err());
    let wrong = EvidenceScope {
        owner_id: "owner-b".into(),
        ..scope
    };
    assert!(b
        .read(ReadRequest {
            scope: wrong.clone(),
            revision: 1
        })
        .await
        .is_err());
    assert!(b
        .freeze(FreezeRequest {
            scope: wrong,
            source_cutoff_ms: 40_000,
            correction_of: None
        })
        .await
        .is_err());
    let left_scope = EvidenceScope {
        window_id: "left".into(),
        from_ms: 40_000,
        to_ms: 43_000,
        ..request.scope.clone()
    };
    let right_scope = EvidenceScope {
        window_id: "right".into(),
        from_ms: 42_000,
        to_ms: 45_000,
        ..request.scope.clone()
    };
    let (left_result, right_result) = tokio::join!(
        a.freeze(FreezeRequest {
            scope: left_scope,
            source_cutoff_ms: 45_000,
            correction_of: None
        }),
        b.freeze(FreezeRequest {
            scope: right_scope,
            source_cutoff_ms: 45_000,
            correction_of: None
        })
    );
    assert_ne!(left_result.is_ok(), right_result.is_ok());
    assert_eq!(a.rows.count_documents(doc! {}).await.unwrap(), 3);
    database.drop().await.unwrap();
}

#[test]
fn measured_batches_history_bound_counts_the_prospective_payload() {
    assert_eq!(
        checked_history_bytes(MAX_BATCH_PAYLOAD, MAX_BATCH_PAYLOAD, MAX_HISTORY_BYTES).unwrap(),
        MAX_HISTORY_BYTES
    );
    assert!(checked_history_bytes(MAX_HISTORY_BYTES, 1, MAX_HISTORY_BYTES).is_err());
    assert!(checked_history_bytes(usize::MAX, 1, MAX_HISTORY_BYTES).is_err());
}
