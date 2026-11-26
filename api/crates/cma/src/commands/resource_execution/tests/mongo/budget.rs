use super::*;
use std::time::Duration;

// Real worker/store recovery; the execution and release receipts are synthetic.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn saturated_worker_cancellation_preserves_full_cohort_for_recovery() {
    let fixture = Fixture::new().await;
    let mut rows = Vec::new();
    for n in 1..=64 {
        let suffix = format!("budget-{n}");
        rows.push(if n % 2 == 0 {
            fixture.release(&suffix).await
        } else {
            fixture.register(&suffix).await
        });
    }
    let directory = Directory::new(&rows);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    for endpoint in directory.0.values() {
        *endpoint.stop_gate.lock().unwrap() = Some(gate.clone());
    }
    let original = fixture.freeze().await;
    let worker = Arc::new(RevocationWorker::new(
        fixture.store.clone(),
        directory.clone(),
    ));
    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let worker = worker.clone();
        let operation = original.key.operation_id.clone();
        attempts.spawn(async move { worker.reconcile(&operation).await });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let row = fixture
                .store
                .record(&rows[0].registration)
                .await
                .unwrap()
                .unwrap();
            assert!(row.completion.is_none());
            if row.barrier.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    attempts.abort_all();
    while let Some(result) = attempts.join_next().await {
        assert!(result.unwrap_err().is_cancelled());
    }
    drop(worker);
    for endpoint in directory.0.values() {
        assert!(endpoint.retained.lock().unwrap().is_none());
    }
    let pending = fixture
        .store
        .revocation(&original.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(pending == original);
    gate.add_permits(64);
    let replacement = RevocationWorker::new(fixture.reopened().await, directory.clone());
    assert_eq!(
        replacement
            .reconcile(&original.key.operation_id)
            .await
            .unwrap(),
        Progress::Effective
    );
    final_operation(&fixture, &original).await;
    for row in rows {
        let retained = fixture
            .store
            .record(&row.registration)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retained.member, row.member);
        assert_eq!(retained.basis, row.basis);
        assert!(retained.barrier.is_some());
        assert!(retained.completion.is_some());
    }
    for endpoint in directory.0.values() {
        assert_eq!(
            endpoint
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|&&call| call == "stop")
                .count(),
            1
        );
    }
    fixture.cleanup().await;
}
