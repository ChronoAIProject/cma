use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::*;

#[test]
fn unknown_versions_include_missing_wrong_and_non_integer_values() {
    assert!(require_version(&doc! { "document_schema_version": 1_i64 }).is_ok());
    assert!(require_version(&doc! { "document_schema_version": 1_i32 }).is_ok());
    assert!(require_version(&doc! { "document_schema_version": 2_i64 }).is_err());
    assert!(require_version(&doc! { "document_schema_version": 1.0 }).is_err());
    assert!(require_version(&Document::new()).is_err());
}

#[test]
fn aggregate_record_round_trip_checks_the_version_and_bson_payload() {
    #[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
    struct Record {
        counter: i64,
    }

    let record = Record { counter: 37 };
    let encoded = record_bson(&record).unwrap();
    assert_eq!(
        encoded,
        mongodb::bson::Bson::Document(doc! { "counter": 37_i64 })
    );
    for version in [mongodb::bson::Bson::Int32(1), mongodb::bson::Bson::Int64(1)] {
        let envelope = doc! { "document_schema_version": version, "record": &encoded };
        assert_eq!(read_record::<Record>(&envelope).unwrap(), record);
    }
    assert_eq!(
        read_record::<Record>(&doc! { "document_schema_version": 2_i32 }).unwrap_err(),
        "unsupported document schema version 2"
    );
    assert_eq!(
        read_record::<Record>(&doc! { "document_schema_version": 1_i32 }).unwrap_err(),
        "stored document has no aggregate record"
    );
    assert!(
        read_record::<Record>(&doc! { "document_schema_version": 1_i64, "record": false }).is_err()
    );
}

#[test]
fn integer_conversion_keeps_the_bson_signed_boundary() {
    assert_eq!(signed(0, "revision"), Ok(0));
    assert_eq!(signed(i64::MAX as u64, "revision"), Ok(i64::MAX));
    assert_eq!(
        signed(i64::MAX as u64 + 1, "revision"),
        Err("revision exceeds the BSON integer range".into())
    );
    assert_eq!(unsigned(i64::MAX, "revision"), Ok(i64::MAX as u64));
    assert_eq!(
        unsigned(-1, "revision"),
        Err("revision must not be negative".into())
    );
}

#[tokio::test(start_paused = true)]
async fn reconciliation_deadline_detaches_instead_of_cancelling_driver_work() {
    let (release, blocked) = tokio::sync::oneshot::channel::<()>();
    let finished = Arc::new(AtomicBool::new(false));
    let completed = Arc::clone(&finished);
    let result = wait_for_reconciliation_with_deadline(
        "test reconciliation",
        Duration::from_secs(1),
        async move {
            let _ = blocked.await;
            completed.store(true, Ordering::SeqCst);
        },
    )
    .await;

    assert_eq!(
        result,
        Err("test reconciliation exceeded its 1 second deadline".into())
    );
    assert!(!finished.load(Ordering::SeqCst));
    release.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(finished.load(Ordering::SeqCst));
}

#[tokio::test]
async fn caller_cancellation_detaches_an_active_reconciliation() {
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
    let caller = tokio::spawn(wait_for_reconciliation("cancelled caller", async move {
        started_tx.send(()).unwrap();
        let _ = release_rx.await;
        finished_tx.send(()).unwrap();
    }));

    started_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
}
