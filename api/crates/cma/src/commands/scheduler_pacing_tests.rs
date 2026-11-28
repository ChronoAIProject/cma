use super::*;
use crate::commands::reconcile_schedule::RECONCILE_AUDIT_INTERVAL;

impl Probe {
    fn dispatched(&self, name: &str, current: bool, behavior: Behavior) -> AgentCommandId {
        let id = self.put(name, behavior);
        self.dispatched.lock().unwrap().insert(id.clone(), current);
        id
    }
    fn paced(self: &Arc<Self>) -> CommandScheduler {
        let mut scheduler = self.scheduler(8);
        scheduler.reconcile_pacing = true;
        scheduler
    }
}

async fn advance(duration: Duration) {
    tokio::time::advance(duration).await;
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn terminal_wake_rechecks_only_the_matching_agents_queued_command() {
    let probe = Arc::new(Probe::default());
    let first = probe.put("cmd_first", Behavior::Deferred(Duration::from_secs(15)));
    let second = probe.put("cmd_second", Behavior::Deferred(Duration::from_secs(15)));
    let first_agent = AgentId("agt_first".into());
    let second_agent = AgentId("agt_second".into());
    probe.turn_waits.lock().unwrap().extend([
        (first.clone(), first_agent.clone()),
        (second.clone(), second_agent),
    ]);
    let (wake, _) = broadcast::channel(8);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(2)
        .with_turn_terminal_wake(wake.clone())
        .spawn(signal);
    settle().await;
    assert_eq!(probe.count(&first), 1);
    assert_eq!(probe.count(&second), 1);

    wake.send(first_agent).unwrap();
    settle().await;
    advance(POLL_INTERVAL).await;
    assert_eq!(probe.count(&first), 2);
    assert_eq!(probe.count(&second), 1);

    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancel_wake_scans_before_the_periodic_tick_and_coalesces_at_fifty_ms() {
    let probe = Arc::new(Probe::default());
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .paced()
        .with_cancel_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    assert_eq!(probe.discovery_calls.lock().unwrap().len(), 1);
    advance(Duration::from_millis(10)).await;

    let committed = Instant::now();
    let first = probe.put("cmd_interrupt_first", Behavior::Complete);
    wake.notify_one();
    advance(Duration::from_millis(1)).await;
    assert_eq!(probe.count(&first), 1);
    let first_wake_scan = probe.discovery_calls.lock().unwrap()[1];
    assert!(first_wake_scan.duration_since(committed) < POLL_INTERVAL);

    let second = probe.put("cmd_interrupt_second", Behavior::Complete);
    for _ in 0..32 {
        wake.notify_one();
    }
    advance(CANCEL_WAKE_SPACING - Duration::from_millis(2)).await;
    assert_eq!(probe.discovery_calls.lock().unwrap().len(), 2);
    assert_eq!(probe.count(&second), 0);
    advance(Duration::from_millis(2)).await;
    assert_eq!(probe.count(&second), 1);
    let second_wake_scan = probe.discovery_calls.lock().unwrap()[2];
    assert!(second_wake_scan.duration_since(first_wake_scan) >= CANCEL_WAKE_SPACING);
    advance(CANCEL_WAKE_SPACING).await;
    assert_eq!(probe.discovery_calls.lock().unwrap().len(), 3);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancel_wake_waits_for_capacity_and_shutdown_drops_pending_discovery() {
    let probe = Arc::new(Probe::default());
    let release = Arc::new(Notify::new());
    let first = probe.put("cmd_existing", Behavior::Wait(release.clone()));
    let wake = Arc::new(Notify::new());
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe
        .scheduler(1)
        .with_cancel_wake(Some(wake.clone()))
        .spawn(signal);
    settle().await;
    assert_eq!(probe.count(&first), 1);

    let waiting = probe.put("cmd_interrupt_waiting", Behavior::Complete);
    wake.notify_one();
    advance(CANCEL_WAKE_SPACING).await;
    assert_eq!(probe.discovery_calls.lock().unwrap().len(), 1);
    assert_eq!(probe.count(&waiting), 0);
    release.notify_one();
    settle().await;
    assert_eq!(probe.count(&waiting), 1);
    assert_eq!(probe.peak.load(Ordering::SeqCst), 1);

    let late = probe.put("cmd_interrupt_shutdown", Behavior::Complete);
    wake.notify_one();
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
    assert_eq!(probe.count(&late), 0);
    assert_eq!(probe.discovery_calls.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn first_fold_is_required_then_unchanged_work_waits_for_the_audit() {
    let probe = Arc::new(Probe::default());
    let id = probe.dispatched("cmd_a", true, Behavior::Deferred(Duration::from_secs(1)));
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.paced().spawn(signal);
    settle().await;
    assert_eq!(probe.count(&id), 1);
    for _ in 0..1_199 {
        advance(POLL_INTERVAL).await;
    }
    assert_eq!(probe.count(&id), 1);
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    advance(POLL_INTERVAL).await;
    assert_eq!(probe.count(&id), 2);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();

    let (shutdown, signal) = watch::channel(false);
    let restarted = probe.paced().spawn(signal);
    settle().await;
    assert_eq!(
        probe.count(&id),
        3,
        "process restart requires a fresh audit"
    );
    shutdown.send(true).unwrap();
    restarted.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn advanced_head_runs_after_cooldown_within_one_discovery_period() {
    let probe = Arc::new(Probe::default());
    let id = probe.dispatched("cmd_a", true, Behavior::Deferred(Duration::from_secs(1)));
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.paced().spawn(signal);
    settle().await;
    probe.dispatched.lock().unwrap().insert(id.clone(), false);
    for _ in 0..3 {
        advance(POLL_INTERVAL).await;
    }
    assert_eq!(probe.count(&id), 1);
    advance(POLL_INTERVAL).await;
    assert_eq!(probe.count(&id), 2);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unchanged_dispatched_commands_leave_slots_for_new_queued_work() {
    let probe = Arc::new(Probe::default());
    let ids: Vec<_> = (0..8)
        .map(|index| {
            probe.dispatched(
                &format!("cmd_{index}"),
                true,
                Behavior::Deferred(Duration::from_secs(1)),
            )
        })
        .collect();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.paced().spawn(signal);
    settle().await;
    advance(Duration::from_secs(1)).await;
    let fresh = probe.put("cmd_new", Behavior::Complete);
    advance(POLL_INTERVAL).await;
    assert_eq!(probe.count(&fresh), 1);
    assert!(ids.iter().all(|id| probe.count(id) == 1));
    assert_eq!(probe.active.load(Ordering::SeqCst), 0);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn reconciliation_errors_back_off_to_five_seconds_then_success_resets() {
    for error in [
        Behavior::Fail,
        Behavior::Conflict,
        Behavior::Stale,
        Behavior::Panic,
    ] {
        let probe = Arc::new(Probe::default());
        let id = probe.dispatched("cmd_a", true, error.clone());
        let (shutdown, signal) = watch::channel(false);
        let scheduler = probe.paced().spawn(signal);
        settle().await;
        let mut expected = 1;
        for millis in [250, 500, 1000, 2000, 4000, 5000, 5000] {
            for _ in 0..millis / 250 - 1 {
                advance(POLL_INTERVAL).await;
            }
            assert_eq!(probe.count(&id), expected);
            advance(POLL_INTERVAL).await;
            expected += 1;
            assert_eq!(probe.count(&id), expected);
        }
        probe
            .jobs
            .lock()
            .unwrap()
            .insert(id.clone(), Behavior::Deferred(Duration::from_secs(1)));
        advance(Duration::from_secs(5)).await;
        expected += 1;
        assert_eq!(probe.count(&id), expected);
        probe.dispatched.lock().unwrap().insert(id.clone(), false);
        probe.jobs.lock().unwrap().insert(id.clone(), error);
        advance(Duration::from_secs(1)).await;
        expected += 1;
        assert_eq!(probe.count(&id), expected);
        advance(POLL_INTERVAL).await;
        assert_eq!(probe.count(&id), expected + 1);
        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn disabled_switch_preserves_zero_delay_and_error_cadence() {
    for behavior in [
        Behavior::Blocked,
        Behavior::Fail,
        Behavior::Conflict,
        Behavior::Stale,
    ] {
        let probe = Arc::new(Probe::default());
        let id = probe.dispatched("cmd_a", true, behavior);
        let (shutdown, signal) = watch::channel(false);
        let scheduler = probe.scheduler(8).spawn(signal);
        settle().await;
        for expected in 2..=5 {
            advance(POLL_INTERVAL).await;
            assert_eq!(probe.count(&id), expected);
        }
        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn queued_errors_keep_the_existing_cadence_with_pacing_enabled() {
    let probe = Arc::new(Probe::default());
    let id = probe.put("cmd_a", Behavior::Fail);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = probe.paced().spawn(signal);
    settle().await;
    for expected in 2..=5 {
        advance(POLL_INTERVAL).await;
        assert_eq!(probe.count(&id), expected);
    }
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
    assert_eq!(RECONCILE_AUDIT_INTERVAL, Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn supervised_recovery_preserves_pacing_and_in_flight_commands() {
    for stall in [false, true] {
        let probe = Arc::new(Probe::default());
        let current = probe.dispatched("cmd_a", true, Behavior::Deferred(Duration::from_secs(1)));
        let release = Arc::new(Notify::new());
        let held = probe.put("cmd_b", Behavior::Wait(release.clone()));
        let activity = crate::worker_supervision::Workers::new().get("agent_commands");
        let (shutdown, signal) = watch::channel(false);
        let scheduler = probe.paced().spawn_supervised(signal, activity.clone());
        settle().await;
        assert_eq!(probe.count(&current), 1);
        assert_eq!(probe.count(&held), 1);
        // Skipping current heads is healthy scheduling, even past the watchdog grace.
        for _ in 0..24 {
            advance(POLL_INTERVAL).await;
            assert!(activity.snapshot().available());
        }
        assert_eq!(probe.count(&current), 1);
        if stall {
            crate::worker_supervision::stall_next_poll(&activity);
        } else {
            crate::worker_supervision::panic_next_poll(&activity);
        }
        advance(POLL_INTERVAL).await;
        if stall {
            advance(Duration::from_secs(6)).await;
        }
        assert_eq!(activity.snapshot().state, "backoff");
        assert!(!activity.snapshot().available());
        assert_eq!(probe.active.load(Ordering::SeqCst), 1);
        let fresh = probe.put("cmd_c", Behavior::Complete);
        advance(Duration::from_secs(1)).await;
        assert!(activity.snapshot().available());
        assert_eq!(probe.count(&fresh), 1);
        assert_eq!(
            probe.count(&held),
            1,
            "recovery must retain the execution child"
        );
        assert_eq!(
            probe.count(&current),
            1,
            "loop recovery must retain the audit cache"
        );
        release.notify_one();
        settle().await;
        assert_eq!(probe.active.load(Ordering::SeqCst), 0);
        probe
            .dispatched
            .lock()
            .unwrap()
            .insert(current.clone(), false);
        advance(POLL_INTERVAL).await;
        assert_eq!(
            probe.count(&current),
            2,
            "an advanced head still reconciles"
        );
        shutdown.send(true).unwrap();
        scheduler.await.unwrap();
        assert_eq!(activity.snapshot().state, "stopped");
    }
}
