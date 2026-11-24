use super::super::super::sweep::{RevocationSweep, SweepEvent};
use super::*;
use std::time::Duration;

async fn reconciled(sweep: &mut RevocationSweep) -> (wire::Digest, ExecutionResult<Progress>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let SweepEvent::Reconciled { operation, result } = sweep.next().await {
                return (operation, result);
            }
        }
    })
    .await
    .unwrap()
}

async fn empty_scan(sweep: &mut RevocationSweep) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let SweepEvent::Discovered { count, end_of_scan } = sweep.next().await {
                assert_eq!(count, 0);
                assert!(end_of_scan);
                break;
            }
        }
    })
    .await
    .unwrap();
}

// Participant receipts are synthetic. The sweeps, workers and Mongo writes are real.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn reopened_sweep_recovers_lost_reply_with_original_operation_and_deadline() {
    let fixture = Fixture::new().await;
    let row = fixture.register("sweep-recovery").await;
    let directory = Directory::new(std::slice::from_ref(&row));
    directory.endpoint(1).behavior.lock().unwrap().lose_reply = true;
    let operation = fixture.freeze().await;
    let mut sweep = RevocationSweep::new(Arc::new(RevocationWorker::new(
        fixture.store.clone(),
        directory.clone(),
    )));
    let (id, progress) = reconciled(&mut sweep).await;
    assert_eq!(id, operation.key.operation_id);
    incomplete(progress.unwrap(), 0, 1);
    assert!(sweep.shutdown().await.is_empty());
    drop(sweep);
    let retained = fixture.store.revocation(&id).await.unwrap().unwrap();
    assert_eq!(retained.key, operation.key);
    assert_eq!(retained.deadline_unix_ms, operation.deadline_unix_ms);
    assert!(matches!(retained.phase, RevocationPhaseV1::Pending));
    let mut reopened = RevocationSweep::new(Arc::new(RevocationWorker::new(
        fixture.reopened().await,
        directory.clone(),
    )));
    assert_eq!(
        reconciled(&mut reopened).await,
        (id, Ok(Progress::Effective))
    );
    let final_state = final_operation(&fixture, &operation).await;
    assert!(fixture.freeze().await == final_state);
    assert_eq!(
        *directory.endpoint(1).calls.lock().unwrap(),
        ["query", "barrier", "stop", "query"]
    );
    empty_scan(&mut reopened).await;
    assert!(reopened.shutdown().await.is_empty());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn cancelled_sweep_wait_retains_child_after_durable_barrier() {
    let fixture = Fixture::new().await;
    let row = fixture.register("sweep-owned-stop").await;
    let directory = Directory::new(std::slice::from_ref(&row));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *directory.endpoint(1).stop_gate.lock().unwrap() = Some(gate.clone());
    let operation = fixture.freeze().await;
    let mut sweep = RevocationSweep::new(Arc::new(RevocationWorker::new(
        fixture.store.clone(),
        directory.clone(),
    )));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if matches!(sweep.next().await, SweepEvent::Discovered { count: 1, .. }) {
                break;
            }
        }
    })
    .await
    .unwrap();
    // Polling starts the owned child. Dropping this scheduling wait leaves it alive.
    let mut wait = Box::pin(sweep.next());
    assert!(futures_util::poll!(wait.as_mut()).is_pending());
    drop(wait);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let retained = fixture
                .store
                .record(&row.registration)
                .await
                .unwrap()
                .unwrap();
            if retained.barrier.is_some() {
                assert!(retained.completion.is_none());
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    for _ in 0..3 {
        assert!(tokio::time::timeout(Duration::from_millis(5), sweep.next())
            .await
            .is_err());
    }
    assert_eq!(
        *directory.endpoint(1).calls.lock().unwrap(),
        ["query", "barrier"]
    );
    gate.add_permits(1);
    assert_eq!(
        reconciled(&mut sweep).await,
        (operation.key.operation_id.clone(), Ok(Progress::Effective))
    );
    final_operation(&fixture, &operation).await;
    assert_eq!(
        *directory.endpoint(1).calls.lock().unwrap(),
        ["query", "barrier", "stop"]
    );
    assert!(sweep.shutdown().await.is_empty());
    fixture.cleanup().await;
}
