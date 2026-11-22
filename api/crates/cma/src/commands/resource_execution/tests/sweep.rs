use super::super::sweep::{RevocationSweep, SweepEvent, SweepWork};
use super::*;
use std::time::Duration;
use tokio::time::{timeout, Instant};

mod fixture;
mod supervision;
use fixture::{operation_id, Work};

async fn event(sweep: &mut RevocationSweep) -> SweepEvent {
    timeout(Duration::from_secs(8), async {
        loop {
            let event = sweep.next().await;
            if !matches!(event, SweepEvent::Waiting) {
                return event;
            }
        }
    })
    .await
    .expect("sweep remains responsive")
}

async fn discover(sweep: &mut RevocationSweep, expected: usize, end: bool) {
    let SweepEvent::Discovered { count, end_of_scan } = event(sweep).await else {
        panic!()
    };
    assert_eq!((count, end_of_scan), (expected, end));
}

#[tokio::test(start_paused = true)]
async fn construction_is_inert_and_empty_scans_are_paced() {
    let work = Work::new(0);
    let mut sweep = RevocationSweep::with_work(work.clone());
    assert!(work.state.lock().unwrap().scans.is_empty());
    assert!(work.state.lock().unwrap().starts.is_empty());
    discover(&mut sweep, 0, true).await;
    let start = Instant::now();
    discover(&mut sweep, 0, true).await;
    assert!(start.elapsed() >= Duration::from_millis(250));
    assert!(sweep.shutdown().await.is_empty());
    assert!(matches!(sweep.next().await, SweepEvent::SchedulingStopped));
}

#[tokio::test(start_paused = true)]
async fn cancelled_waits_preserve_children_and_bound_admission() {
    let work = Work::new(40);
    work.state.lock().unwrap().held = (1..=40).map(operation_id).collect();
    let mut sweep = RevocationSweep::with_work(work.clone());
    discover(&mut sweep, 32, false).await;
    for _ in 0..3 {
        assert!(timeout(Duration::from_millis(1), sweep.next())
            .await
            .is_err());
        let state = work.state.lock().unwrap();
        assert_eq!(state.starts.len(), 4);
        assert_eq!(state.active.len(), 4);
        assert_eq!(state.scans.len(), 1);
        assert!(state.cancelled.is_empty());
    }
    work.release();
    let result = sweep.shutdown().await;
    assert_eq!(result.len(), 4);
    assert!(result
        .iter()
        .all(|(_, result)| *result == Ok(Progress::Effective)));
    assert_eq!(
        work.state.lock().unwrap().starts.len(),
        4,
        "queued work cannot start during drain"
    );
    assert!(sweep.shutdown().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn held_first_page_operation_does_not_block_next_page() {
    let work = Work::new(40);
    work.state.lock().unwrap().held.insert(operation_id(1));
    let mut sweep = RevocationSweep::with_work(work.clone());
    let mut completed = std::collections::HashSet::new();
    for _ in 0..100 {
        if let SweepEvent::Reconciled { operation, result } = event(&mut sweep).await {
            assert_eq!(result, Ok(Progress::Effective));
            completed.insert(operation);
        }
        if completed.contains(&operation_id(40)) {
            break;
        }
    }
    assert!(completed.contains(&operation_id(40)));
    {
        let state = work.state.lock().unwrap();
        assert!(state.active.contains(&operation_id(1)));
        assert!(!completed.contains(&operation_id(1)));
        assert!(state.peak <= 4);
        assert!(state.scans.contains(&Some(operation_id(32))));
        assert_eq!(
            state
                .starts
                .iter()
                .filter(|id| **id == operation_id(1))
                .count(),
            1
        );
    }
    work.release();
    sweep.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn discovery_failure_preserves_cursor_and_end_wraps_to_failed_operations() {
    let work = Work::new(33);
    {
        let mut state = work.state.lock().unwrap();
        state.errors = state.pending.iter().cloned().collect();
        state.fail_scan = Some(2);
    }
    let mut sweep = RevocationSweep::with_work(work.clone());
    let mut failed_discovery = false;
    let mut last_operation_seen = false;
    for _ in 0..100 {
        match event(&mut sweep).await {
            SweepEvent::DiscoveryFailed(error) => {
                assert_eq!(error, ExecutionError::Unavailable);
                failed_discovery = true;
            }
            SweepEvent::Reconciled { operation, result } => {
                assert_eq!(result, Err(ExecutionError::Unknown));
                last_operation_seen |= operation == operation_id(33);
            }
            _ => {}
        }
        if work.state.lock().unwrap().scans.len() >= 4 {
            break;
        }
    }
    assert!(failed_discovery && last_operation_seen);
    assert_eq!(
        work.state.lock().unwrap().scans[..4],
        [None, Some(operation_id(32)), Some(operation_id(32)), None]
    );
    sweep.shutdown().await;
    let mut restarted = RevocationSweep::with_work(work.clone());
    discover(&mut restarted, 32, false).await;
    assert!(work.state.lock().unwrap().scans.last().unwrap().is_none());
    restarted.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn panicked_child_reports_its_original_identity_as_unknown() {
    let work = Work::new(2);
    work.state.lock().unwrap().panics.insert(operation_id(1));
    let mut sweep = RevocationSweep::with_work(work.clone());
    discover(&mut sweep, 2, true).await;
    let mut results = std::collections::BTreeMap::new();
    for _ in 0..10 {
        if let SweepEvent::Reconciled { operation, result } = event(&mut sweep).await {
            results.insert(operation, result);
        }
        if results.len() == 2 {
            break;
        }
    }
    assert_eq!(results[&operation_id(1)], Err(ExecutionError::Unknown));
    assert_eq!(results[&operation_id(2)], Ok(Progress::Effective));
    assert!(work
        .state
        .lock()
        .unwrap()
        .pending
        .contains(&operation_id(1)));
    sweep.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn discovery_timeout_never_admits_or_completes_work() {
    let work = Work::new(1);
    work.state.lock().unwrap().stall_scan = true;
    let mut sweep = RevocationSweep::with_work(work.clone());
    let start = Instant::now();
    assert!(matches!(
        event(&mut sweep).await,
        SweepEvent::DiscoveryFailed(ExecutionError::Unavailable)
    ));
    assert_eq!(start.elapsed(), Duration::from_secs(5));
    assert!(work.state.lock().unwrap().starts.is_empty());
    assert!(sweep.shutdown().await.is_empty());
}

#[tokio::test(start_paused = true)]
async fn cancelled_shutdown_retains_results_deadline_and_closed_admission() {
    let work = Work::new(10);
    work.state.lock().unwrap().held = (2..=10).map(operation_id).collect();
    let mut sweep = RevocationSweep::with_work(work.clone());
    discover(&mut sweep, 10, true).await;
    // Start four children without consuming their results in the scheduling loop.
    let mut wait = Box::pin(sweep.next());
    assert!(futures_util::poll!(wait.as_mut()).is_pending());
    drop(wait);
    tokio::task::yield_now().await;
    let start = Instant::now();
    assert!(timeout(Duration::from_millis(10), sweep.shutdown())
        .await
        .is_err());
    assert!(matches!(sweep.next().await, SweepEvent::SchedulingStopped));
    let results = sweep.shutdown().await;
    assert_eq!(start.elapsed(), Duration::from_secs(25));
    assert_eq!(results.len(), 4);
    assert_eq!(
        results
            .iter()
            .find(|(id, _)| *id == operation_id(1))
            .unwrap()
            .1,
        Ok(Progress::Effective)
    );
    assert!(results
        .iter()
        .filter(|(id, _)| *id != operation_id(1))
        .all(|(_, result)| *result == Err(ExecutionError::Unknown)));
    assert_eq!(work.state.lock().unwrap().starts.len(), 4);
    assert_eq!(work.state.lock().unwrap().cancelled.len(), 3);
    assert!(sweep.shutdown().await.is_empty());
}
