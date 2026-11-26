use super::*;
use std::time::Duration;
use tokio::sync::Semaphore;

fn no_participant_calls(directory: &Directory) {
    assert!(directory
        .0
        .values()
        .all(|endpoint| endpoint.calls.lock().unwrap().is_empty()));
}

#[tokio::test(start_paused = true)]
async fn capture_deadline_spans_operation_and_all_cohort_pages() {
    for blocked_page in [None, Some(0), Some(32)] {
        let (store, directory, worker) = setup((1..=33).map(member).collect());
        let original = store.0.lock().unwrap().operation.clone();
        let original_members = store.0.lock().unwrap().members.clone();
        let operation_gate = Arc::new(Semaphore::new(0));
        {
            let mut state = store.0.lock().unwrap();
            state.operation_gate = Some(operation_gate.clone());
            state.cohort_page_gate = blocked_page.map(|page| (page, Arc::new(Semaphore::new(0))));
        }
        let mut attempt = Box::pin(worker.reconcile(&original.key.operation_id));
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(4)).await;
        if blocked_page.is_some() {
            operation_gate.add_permits(1);
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
            assert_eq!(store.0.lock().unwrap().pages.last(), blocked_page.as_ref());
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        let std::task::Poll::Ready(result) = futures_util::poll!(attempt.as_mut()) else {
            panic!("operation and cohort reads must share one bounded capture window")
        };
        assert_eq!(result, Err(ExecutionError::Unavailable));
        no_participant_calls(&directory);
        {
            let mut state = store.0.lock().unwrap();
            assert!(state.operation == original);
            assert!(state.members == original_members);
            assert_eq!(state.finalizations, 0);
            state.operation_gate = None;
            state.cohort_page_gate = None;
        }
        assert_eq!(
            worker.reconcile(&original.key.operation_id).await.unwrap(),
            Progress::Effective
        );
        assert_eq!(store.id(), original.key.operation_id);
    }
}

#[tokio::test(start_paused = true)]
async fn finalization_timeout_preserves_uncommitted_and_committed_outcomes() {
    for committed in [false, true] {
        let (store, directory, worker) = setup(vec![member(1), release_member(2)]);
        let original = store.0.lock().unwrap().operation.clone();
        let gate = Some(Arc::new(Semaphore::new(0)));
        if committed {
            store.0.lock().unwrap().finalize_reply_gate = gate;
        } else {
            store.0.lock().unwrap().finalize_before_gate = gate;
        }
        let mut attempt = Box::pin(worker.reconcile(&original.key.operation_id));
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(2)).await;
        let std::task::Poll::Ready(result) = futures_util::poll!(attempt.as_mut()) else {
            panic!("a missing finalization reply must release the sweep slot as unknown")
        };
        assert_eq!(result, Err(ExecutionError::Unknown));
        let retained = {
            let mut state = store.0.lock().unwrap();
            assert_eq!(
                matches!(state.operation.phase, RevocationPhaseV1::Effective { .. }),
                committed
            );
            assert_eq!(state.operation.key, original.key);
            assert!(state.operation.intent == original.intent);
            assert_eq!(state.operation.deadline_unix_ms, original.deadline_unix_ms);
            assert!(state
                .members
                .iter()
                .all(|member| member.completion.is_some()));
            assert_eq!(state.finalizations, usize::from(committed));
            state.finalize_reply_gate = None;
            state.finalize_before_gate = None;
            state.operation.clone()
        };
        let replacement = RevocationWorker::new(store.clone(), directory.clone());
        assert_eq!(
            replacement
                .reconcile(&original.key.operation_id)
                .await
                .unwrap(),
            Progress::Effective
        );
        let state = store.0.lock().unwrap();
        assert_eq!(state.finalizations, 1);
        if committed {
            assert!(state.operation == retained);
        }
        for endpoint in directory.0.values() {
            assert_eq!(
                *endpoint.calls.lock().unwrap(),
                ["query", "barrier", "stop"]
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn slow_capture_does_not_shorten_the_participant_observation_window() {
    let (store, directory, worker) = setup(vec![member(1)]);
    let operation_gate = Arc::new(Semaphore::new(0));
    store.0.lock().unwrap().operation_gate = Some(operation_gate.clone());
    directory.endpoint(1).behavior.lock().unwrap().stall = true;
    let original = store.id();
    let mut attempt = Box::pin(worker.reconcile(&original));
    assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    tokio::time::advance(Duration::from_secs(4)).await;
    operation_gate.add_permits(1);
    assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    assert_eq!(*directory.endpoint(1).calls.lock().unwrap(), ["query"]);
    tokio::time::advance(Duration::from_millis(1_500)).await;
    assert!(futures_util::poll!(attempt.as_mut()).is_pending());
    tokio::time::advance(Duration::from_millis(500)).await;
    pending(attempt.await.unwrap(), 0);
    assert_eq!(store.0.lock().unwrap().finalizations, 0);
    assert_eq!(store.id(), original);
}

mod sweep;
