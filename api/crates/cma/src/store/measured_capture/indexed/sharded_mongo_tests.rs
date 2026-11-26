use super::*;
use crate::store::measured_capture::{
    indexed::{sharded_generation, store_error},
    CaptureError, TransactionError,
};
use crate::store::sandbox_mongo::start_transaction;
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::sync::Arc;
use tokio::sync::Barrier;

async fn create_custody(database: &Database) {
    for name in [
        crate::store::migrations::measured_custody::COLLECTION,
        MANIFESTS,
        PROJECTIONS,
    ] {
        database.create_collection(name).await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn sharded_index_handoff_reconciles_previous_custody_and_refuses_missing_membership() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_index_handoff_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    create_custody(&database).await;
    let index = IndexedCustody::new(&database, "a".repeat(64)).unwrap();
    let capture = MeasuredCapture::new(&database).with_indexed(index.clone());
    let mut binding = fixture_binding("owner-a");
    binding.valid_from = 3500;
    binding.observed_at = 3800;
    let previous = observation(&binding, 3_599_000, 20.0);
    let next = observation(&binding, 3_601_000, 25.0);

    index.begin().await.unwrap();
    index.certify().await.unwrap();
    assert_eq!(capture.capture(&previous).await.unwrap(), Capture::Inserted);
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    let old_root = index
        .root(&mut snapshot, true)
        .await
        .map_err(store_error)
        .unwrap();
    let previous_digest = digest(&old_root).unwrap();
    assert!(matches!(
        index.begin_current(Some(&"b".repeat(64))).await,
        Err(CaptureError::Unavailable)
    ));
    let mut stale = client.start_session().await.unwrap();
    start_transaction(&mut stale).await.unwrap();
    assert_eq!(
        index
            .root(&mut stale, false)
            .await
            .map_err(store_error)
            .unwrap(),
        old_root
    );
    index.begin_current(Some(&previous_digest)).await.unwrap();
    assert!(capture
        .apply(
            &next,
            &next.evidence_sha256().unwrap(),
            &serde_json::to_string(&next).unwrap(),
            &mut stale,
        )
        .await
        .is_err());
    stale.abort_guarded().await.unwrap();
    assert!(matches!(
        capture.capture(&next).await,
        Err(CaptureError::Unavailable)
    ));
    assert_eq!(index.originals.count_documents(doc! {}).await.unwrap(), 1);
    assert!(index.certify().await.is_err());
    assert_eq!(index.advance().await.unwrap(), (1, false));
    assert_eq!(index.advance().await.unwrap(), (1, true));
    assert_eq!(index.certify().await.unwrap().original_rows, 1);
    index.begin_current(Some(&previous_digest)).await.unwrap();
    assert_eq!(capture.capture(&previous).await.unwrap(), Capture::Replay);
    assert_eq!(capture.capture(&next).await.unwrap(), Capture::Inserted);
    assert_eq!(index.originals.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        index
            .projections
            .count_documents(doc! {"generation": 1_i64})
            .await
            .unwrap(),
        1
    );

    let window = scope("two-hours", 3_598_000, 3_602_000);
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    assert_eq!(
        index
            .read_scope(&window, 3_800_000, &mut snapshot)
            .await
            .unwrap()
            .len(),
        2
    );
    let extra_shard = sharded_generation::shard_id(2, 17);
    assert_eq!(
        index
            .manifests
            .delete_one(doc! {"_id": extra_shard})
            .await
            .unwrap()
            .deleted_count,
        1
    );
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        index.read_scope(&window, 3_800_000, &mut snapshot).await,
        Err(CaptureError::Corrupt)
    ));
    database.drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn sharded_index_captures_one_hot_physical_series_through_independent_replicas() {
    const WRITERS: u64 = 16;
    const START: u64 = 3_601_000;
    let rows_per_writer = MAX_SERIES_HOUR_ROWS as u64 / WRITERS;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("measured_index_hot_{}", uuid::Uuid::new_v4().simple());
    let database = client.database(&name);
    create_custody(&database).await;
    let index = IndexedCustody::new(&database, "a".repeat(64)).unwrap();
    index.begin_current(None).await.unwrap();
    assert_eq!(index.certify().await.unwrap().original_rows, 0);
    assert!(matches!(
        index
            .root(&mut client.start_session().await.unwrap(), false)
            .await,
        Err(TransactionError::Store(CaptureError::Unavailable))
    ));

    let mut binding = fixture_binding("owner-a");
    binding.valid_from = 3500;
    binding.observed_at = 3800;
    let barrier = Arc::new(Barrier::new(WRITERS as usize));
    let mut tasks = Vec::new();
    for worker in 0..WRITERS {
        let database = Client::with_uri_str(&uri).await.unwrap().database(&name);
        let peer = IndexedCustody::new(&database, "a".repeat(64)).unwrap();
        let capture = MeasuredCapture::new(&database).with_indexed(peer);
        let binding = binding.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            for row in 0..rows_per_writer {
                let point = observation(&binding, START + worker * rows_per_writer + row, 20.0);
                assert_eq!(capture.capture(&point).await.unwrap(), Capture::Inserted);
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let mut snapshot = client.start_session().snapshot(true).await.unwrap();
    let window = scope("hot-series", 3_600_000, 3_604_000);
    assert_eq!(
        index
            .read_scope(&window, 3_800_000, &mut snapshot)
            .await
            .unwrap()
            .len(),
        MAX_SERIES_HOUR_ROWS
    );
    let retained_count = MAX_SERIES_HOUR_ROWS as u64;
    assert_eq!(
        index.originals.count_documents(doc! {}).await.unwrap(),
        retained_count
    );
    assert_eq!(
        index
            .projections
            .count_documents(doc! {"generation": 1_i64})
            .await
            .unwrap(),
        retained_count
    );
    let capture = MeasuredCapture::new(&database).with_indexed(index.clone());
    let overflow = observation(&binding, START + retained_count, 20.0);
    assert_eq!(
        capture
            .capture(&observation(&binding, START, 20.0))
            .await
            .unwrap(),
        Capture::Replay
    );
    assert!(matches!(
        capture.capture(&overflow).await,
        Err(CaptureError::Unavailable)
    ));
    let original = index.originals.find_one(doc! {}).await.unwrap().unwrap();
    let point = Projection::from_source(1, &original).unwrap();
    let series = &point.keys.physical_series_id;
    let hour = point.hour();
    let mut permit_ids = vec![sharded_generation::permits_id(1, hour, series)];
    permit_ids.extend((0..64).map(|slot| sharded_generation::segment_id(1, hour, series, slot)));
    assert!(
        index
            .manifests
            .delete_many(doc! {"_id": {"$in": permit_ids}})
            .await
            .unwrap()
            .deleted_count
            > 1
    );
    assert!(matches!(
        capture.capture(&overflow).await,
        Err(CaptureError::Corrupt)
    ));
    assert_eq!(
        index.originals.count_documents(doc! {}).await.unwrap(),
        retained_count
    );
    database.drop().await.unwrap();
}
