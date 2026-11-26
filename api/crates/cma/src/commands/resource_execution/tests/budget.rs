use super::*;
use std::time::Duration;

fn calls(directory: &Directory, name: &str) -> usize {
    directory
        .0
        .values()
        .map(|endpoint| {
            endpoint
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|&&call| call == name)
                .count()
        })
        .sum()
}

#[tokio::test(start_paused = true)]
async fn concurrent_reconciliations_share_capacity_until_stop_or_cancellation() {
    let (store, directory, worker) = setup(
        (1..=64)
            .map(|n| {
                if n % 2 == 0 {
                    release_member(n)
                } else {
                    member(n)
                }
            })
            .collect(),
    );
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    for endpoint in directory.0.values() {
        *endpoint.stop_gate.lock().unwrap() = Some(gate.clone());
    }
    let id = store.id();
    let mut attempts: Vec<_> = (0..4).map(|_| Box::pin(worker.reconcile(&id))).collect();
    for _ in 0..8 {
        for attempt in &mut attempts {
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(calls(&directory, "query"), 16);
    assert_eq!(calls(&directory, "barrier"), 16);
    assert_eq!(calls(&directory, "stop"), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);

    // Cancelling the active attempt frees slots for already queued attempts.
    drop(attempts.remove(0));
    for _ in 0..8 {
        for attempt in &mut attempts {
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(calls(&directory, "query"), 32);
    assert_eq!(calls(&directory, "stop"), 0);
    drop(attempts);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    gate.add_permits(64);
    assert_eq!(worker.reconcile(&id).await.unwrap(), Progress::Effective);
    assert_eq!(calls(&directory, "stop"), 64);
    assert_eq!(store.id(), id);
}

#[tokio::test(start_paused = true)]
async fn queued_attempt_receives_full_observation_window_after_capacity_wait() {
    let (store, directory, worker) = setup((1..=16).map(member).collect());
    for endpoint in directory.0.values() {
        endpoint.behavior.lock().unwrap().stall = true;
    }
    let id = store.id();
    let mut first = Box::pin(worker.reconcile(&id));
    let mut queued = Box::pin(worker.reconcile(&id));
    assert!(futures_util::poll!(first.as_mut()).is_pending());
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    assert_eq!(calls(&directory, "query"), 16);
    tokio::time::advance(Duration::from_secs(2)).await;
    pending(first.await.unwrap(), 0);
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    assert_eq!(calls(&directory, "query"), 32);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    pending(queued.await.unwrap(), 0);
    assert_eq!(calls(&directory, "barrier"), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);

    for endpoint in directory.0.values() {
        endpoint.behavior.lock().unwrap().stall = false;
    }
    assert_eq!(worker.reconcile(&id).await.unwrap(), Progress::Effective);
    assert_eq!(calls(&directory, "stop"), 16);
}

#[tokio::test]
async fn failed_observations_return_capacity_for_the_rest_of_the_cohort() {
    let (store, directory, worker) = setup((1..=64).map(member).collect());
    for endpoint in directory.0.values() {
        endpoint.behavior.lock().unwrap().unavailable = true;
    }
    let id = store.id();
    let failed = tokio::time::timeout(Duration::from_secs(2), worker.reconcile(&id))
        .await
        .unwrap()
        .unwrap();
    pending(failed, 0);
    assert_eq!(calls(&directory, "query"), 64);
    assert_eq!(calls(&directory, "barrier"), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    for endpoint in directory.0.values() {
        endpoint.behavior.lock().unwrap().unavailable = false;
    }
    assert_eq!(worker.reconcile(&id).await.unwrap(), Progress::Effective);
    assert_eq!(calls(&directory, "stop"), 64);
}
