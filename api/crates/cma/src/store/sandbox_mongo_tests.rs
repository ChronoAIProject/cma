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
fn transaction_server_requires_topology_and_snapshot_support() {
    assert!(
        validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 13_i32 }).is_ok()
    );
    assert!(
        validate_transaction_server(&doc! { "msg": "isdbgrid", "maxWireVersion": 25_i64 }).is_ok()
    );

    let standalone = validate_transaction_server(&doc! { "maxWireVersion": 25_i32 })
        .unwrap_err()
        .to_string();
    assert!(standalone.contains("replica set or mongos"));
    let old = validate_transaction_server(&doc! { "setName": "rs0", "maxWireVersion": 12_i32 })
        .unwrap_err()
        .to_string();
    assert!(old.contains("MongoDB 5.0 or later"));
    let missing = validate_transaction_server(&doc! { "setName": "rs0" })
        .unwrap_err()
        .to_string();
    assert!(missing.contains("maxWireVersion"));
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
