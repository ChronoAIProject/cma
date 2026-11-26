use super::*;
use crate::worker_supervision::{self, Activity, Workers};
use tokio::sync::watch;

async fn settle() {
    for _ in 0..48 {
        tokio::task::yield_now().await;
    }
}

async fn advance(duration: Duration) {
    tokio::time::advance(duration).await;
    settle().await;
}

fn snapshot(activity: &Activity) -> serde_json::Value {
    serde_json::to_value(activity.snapshot()).unwrap()
}

// The production activity registry remains unchanged until R3 is composed.
fn activity() -> Activity {
    Workers::new().get("agent_commands")
}

#[tokio::test(start_paused = true)]
async fn supervised_scheduler_faults_retain_children_cursor_and_busy_observation() {
    for stall in [false, true] {
        let work = Work::new(40);
        work.state.lock().unwrap().held = (1..=40).map(operation_id).collect();
        let activity = activity();
        let (stop, signal) = watch::channel(false);
        let supervisor =
            RevocationSweep::with_work(work.clone()).spawn_supervised(signal, activity.clone());
        settle().await;
        advance(Duration::from_millis(250)).await;
        assert!(activity.snapshot().available());
        assert_eq!(work.state.lock().unwrap().starts.len(), 4);
        assert!(snapshot(&activity)["reconciliation_age_ms"].is_number());
        if stall {
            worker_supervision::stall_next_poll(&activity);
        } else {
            worker_supervision::panic_next_poll(&activity);
        }
        advance(Duration::from_millis(250)).await;
        if stall {
            advance(Duration::from_secs(6)).await;
        }
        assert_eq!(activity.snapshot().state, "backoff");
        assert!(!activity.snapshot().available());
        assert!(snapshot(&activity)["reconciliation_age_ms"].is_number());
        {
            let state = work.state.lock().unwrap();
            assert_eq!(state.active.len(), 4);
            assert_eq!(state.starts.len(), 4);
            assert_eq!(state.scans, [None]);
            assert!(state.cancelled.is_empty());
        }
        advance(Duration::from_secs(1)).await;
        assert_eq!(activity.snapshot().state, "starting");
        assert!(!activity.snapshot().available());
        advance(Duration::from_millis(250)).await;
        assert!(activity.snapshot().available());
        assert_eq!(work.state.lock().unwrap().starts.len(), 4);
        work.release();
        settle().await;
        advance(Duration::from_millis(250)).await;
        {
            let state = work.state.lock().unwrap();
            assert!(state.pending.is_empty());
            assert_eq!(state.starts.len(), 40);
            assert!(state.scans.contains(&Some(operation_id(32))));
            assert!(state.cancelled.is_empty());
            assert!(state.peak <= 4);
        }
        stop.send(true).unwrap();
        supervisor.await.unwrap();
        assert_eq!(activity.snapshot().state, "stopped");
        assert!(snapshot(&activity)["reconciliation_age_ms"].is_null());
        assert!(snapshot(&activity)["discovery_age_ms"].is_null());
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_supervisor_backoff_drains_without_admitting_queued_work() {
    let work = Work::new(40);
    work.state.lock().unwrap().held = (1..=40).map(operation_id).collect();
    let activity = activity();
    let (stop, signal) = watch::channel(false);
    let supervisor =
        RevocationSweep::with_work(work.clone()).spawn_supervised(signal, activity.clone());
    settle().await;
    worker_supervision::panic_next_poll(&activity);
    advance(Duration::from_millis(250)).await;
    assert_eq!(activity.snapshot().state, "backoff");
    stop.send(true).unwrap();
    work.release();
    supervisor.await.unwrap();
    let state = work.state.lock().unwrap();
    assert_eq!(state.starts.len(), 4);
    assert_eq!(state.pending.len(), 36);
    assert!(state.active.is_empty());
    assert!(state.cancelled.is_empty());
    assert_eq!(activity.snapshot().state, "stopped");
}

#[test]
fn supervised_drain_reports_aborted_operations_as_unknown() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(async {
                let work = Work::new(8);
                work.state.lock().unwrap().held = (1..=8).map(operation_id).collect();
                let activity = activity();
                let (stop, signal) = watch::channel(false);
                let supervisor = RevocationSweep::with_work(work.clone())
                    .spawn_supervised(signal, activity.clone());
                settle().await;
                assert_eq!(work.state.lock().unwrap().starts.len(), 4);
                worker_supervision::panic_next_poll(&activity);
                advance(Duration::from_millis(250)).await;
                assert_eq!(activity.snapshot().state, "backoff");
                assert!(handle
                    .render()
                    .contains("cma_worker_local_active{worker=\"resource_revocations\"} 4"));
                stop.send(true).unwrap();
                settle().await;
                assert_eq!(activity.snapshot().state, "draining");
                assert!(handle
                    .render()
                    .contains("cma_worker_local_active{worker=\"resource_revocations\"} 4"));
                let start = Instant::now();
                supervisor.await.unwrap();
                assert_eq!(start.elapsed(), Duration::from_secs(25));
                let state = work.state.lock().unwrap();
                assert_eq!(state.starts.len(), 4);
                assert_eq!(state.cancelled.len(), 4);
                assert!(state.active.is_empty());
                assert_eq!(state.pending.len(), 8);
                assert_eq!(activity.snapshot().state, "stopped");
                assert!(snapshot(&activity)["reconciliation_age_ms"].is_null());
            });
    });
    let metrics = handle.render();
    assert!(metrics.contains("cma_worker_local_active{worker=\"resource_revocations\"} 0"));
    let outcomes: Vec<_> = metrics
        .lines()
        .filter(|line| {
            line.starts_with("cma_worker_operations_total{")
                && line.contains("worker=\"resource_revocations\"")
                && line.contains("phase=\"reconciliation\"")
        })
        .collect();
    assert_eq!(outcomes.len(), 1, "{metrics}");
    assert!(outcomes[0].contains("outcome=\"unknown\""), "{metrics}");
    assert!(outcomes[0].ends_with(" 4"), "{metrics}");
}

#[tokio::test]
async fn shutdown_before_supervisor_start_never_discovers_or_admits() {
    for closed_sender in [false, true] {
        let work = Work::new(1);
        let activity = activity();
        let (stop, signal) = watch::channel(false);
        if !closed_sender {
            stop.send(true).unwrap();
        }
        drop(stop);
        RevocationSweep::with_work(work.clone())
            .spawn_supervised(signal, activity.clone())
            .await
            .unwrap();
        assert!(work.state.lock().unwrap().scans.is_empty());
        assert!(work.state.lock().unwrap().starts.is_empty());
        assert_eq!(activity.snapshot().state, "stopped");
    }
}

#[tokio::test(start_paused = true)]
async fn supervised_discovery_observation_ends_with_its_owned_query() {
    let work = Work::new(1);
    work.state.lock().unwrap().stall_scan = true;
    let activity = activity();
    let (stop, signal) = watch::channel(false);
    let supervisor =
        RevocationSweep::with_work(work.clone()).spawn_supervised(signal, activity.clone());
    settle().await;
    assert!(snapshot(&activity)["discovery_age_ms"].is_number());
    stop.send(true).unwrap();
    let start = Instant::now();
    supervisor.await.unwrap();
    assert_eq!(start.elapsed(), Duration::from_secs(5));
    assert!(snapshot(&activity)["discovery_age_ms"].is_null());
    assert!(work.state.lock().unwrap().starts.is_empty());
    assert_eq!(work.state.lock().unwrap().scans, [None]);
    assert_eq!(activity.snapshot().state, "stopped");
}
