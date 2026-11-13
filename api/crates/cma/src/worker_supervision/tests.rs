use super::*;

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn suspended_scheduling_future_recovers_without_cancelling_or_replaying_children() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let started = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let effect = started.clone();
    let marker = Dropped(dropped.clone());
    let mut children = tokio::task::JoinSet::new();
    children.spawn(async move {
        let _marker = marker;
        effect.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
    });
    let activity = Workers::new().get("agents");
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        (0, children),
        activity.clone(),
        signal,
        |(attempts, children), activity, shutdown| {
            Box::pin(async move {
                *attempts += 1;
                activity.scheduled();
                activity.waiting(Duration::from_secs(1), "execution_slots");
                if *attempts == 1 {
                    std::future::pending::<()>().await;
                }
                assert_eq!(children.len(), 1);
                crate::shutdown::signalled(shutdown).await;
                children.shutdown().await;
            })
        },
        |_| Box::pin(async {}),
    );
    settle().await;
    assert!(activity.snapshot().available());
    tokio::time::advance(Duration::from_secs(6)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    assert!(!activity.snapshot().available());
    assert!(!dropped.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(activity.snapshot().available());
    assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(started.load(Ordering::SeqCst), 1);
    println!(
        "suspended scheduling: detection=6s; safe loop recovery=7s; original effect child retained (paused time)"
    );
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(activity.snapshot().state, "stopped");
}

#[tokio::test(start_paused = true)]
async fn panic_restarts_with_owned_children_and_queue_intact() {
    struct Owned {
        attempts: usize,
        children: tokio::task::JoinSet<usize>,
        queued: VecDeque<usize>,
    }
    let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let mut children = tokio::task::JoinSet::new();
    let child_release = release.clone();
    children.spawn(async move {
        child_release.notified().await;
        7
    });
    let activity = Workers::new().get("agents");
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        Owned {
            attempts: 0,
            children,
            queued: VecDeque::from([9]),
        },
        activity.clone(),
        signal,
        move |state, activity, shutdown| {
            let send = send.clone();
            Box::pin(async move {
                state.attempts += 1;
                if state.attempts == 1 {
                    panic!("top-level scheduler fault");
                }
                activity.scheduled();
                activity.waiting(Duration::from_secs(1), "execution_slots");
                assert_eq!(state.queued.pop_front(), Some(9));
                assert_eq!(state.children.len(), 1);
                let result = state.children.join_next().await.unwrap().unwrap();
                send.send(result).unwrap();
                crate::shutdown::signalled(shutdown).await;
            })
        },
        |_| Box::pin(async {}),
    );
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    release.notify_one();
    settle().await;
    assert_eq!(receive.try_recv().unwrap(), 7);
    println!(
        "scheduler panic: observed immediately; retained child result after 1s backoff (paused time)"
    );
    assert!(receive.try_recv().is_err());
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn repeated_exits_are_rate_limited_and_shutdown_during_backoff_never_restarts() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let activity = Workers::new().get("launches");
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        count.clone(),
        activity.clone(),
        signal,
        |count, _, _| {
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
            })
        },
        |_| Box::pin(async {}),
    );
    settle().await;
    for delay in [1, 2, 4, 8] {
        tokio::time::advance(Duration::from_secs(delay)).await;
        settle().await;
    }
    assert_eq!(count.load(Ordering::SeqCst), 5);
    assert_eq!(activity.snapshot().state, "restart_limited");
    tokio::time::advance(Duration::from_secs(30)).await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 5);
    stop.send(true).unwrap();
    worker.await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 5);
}

#[tokio::test(start_paused = true)]
async fn cooldown_expires_once_and_repeated_failure_starts_a_new_bounded_window() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let activity = Workers::new().get("launches");
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        count.clone(),
        activity.clone(),
        signal,
        |count, _, _| {
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
            })
        },
        |_| Box::pin(async {}),
    );
    settle().await;
    for delay in [1, 2, 4, 8] {
        tokio::time::advance(Duration::from_secs(delay)).await;
        settle().await;
    }
    tokio::time::advance(RESTART_WINDOW - Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 5);
    assert_eq!(activity.snapshot().state, "restart_limited");
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 6);
    assert_eq!(activity.snapshot().state, "backoff");
    // All failures from the old window have expired; the new first delay is 1s.
    for (delay, expected) in [(1, 7), (2, 8), (4, 9), (8, 10)] {
        tokio::time::advance(Duration::from_secs(delay)).await;
        settle().await;
        assert_eq!(count.load(Ordering::SeqCst), expected);
    }
    assert_eq!(activity.snapshot().state, "restart_limited");
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn successful_retry_after_cooldown_requires_a_turn_and_resets_old_failure_backoff() {
    let activity = Workers::new().get("launches");
    let (stop, signal) = watch::channel(false);
    let worker = spawn(
        0,
        activity.clone(),
        signal,
        |attempt, activity, shutdown| {
            Box::pin(async move {
                *attempt += 1;
                if *attempt <= 5 {
                    return;
                }
                let mut tick = tokio::time::interval(Duration::from_secs(1));
                loop {
                    activity.waiting(Duration::from_secs(1), "idle_scan");
                    tokio::select! {
                        _ = tick.tick() => activity.scheduled(),
                        _ = crate::shutdown::signalled(shutdown.clone()) => return,
                    }
                }
            })
        },
        |_| Box::pin(async {}),
    );
    settle().await;
    for delay in [1, 2, 4, 8] {
        tokio::time::advance(Duration::from_secs(delay)).await;
        settle().await;
    }
    assert!(!activity.snapshot().available());
    tokio::time::advance(RESTART_WINDOW).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "running");
    for _ in 0..61 {
        tokio::time::advance(Duration::from_secs(1)).await;
        settle().await;
        assert!(activity.snapshot().available());
    }
    panic_next_poll(&activity);
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert!(activity.snapshot().available());
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn discovery_deadline_releases_query_and_allows_later_scans() {
    let activity = Workers::new().get("agents");
    let scan = tokio::spawn(discover(
        Some(activity.clone()),
        std::future::pending::<Result<(), ()>>(),
        (),
    ));
    settle().await;
    tokio::time::advance(DISCOVERY_DEADLINE).await;
    settle().await;
    assert_eq!(scan.await.unwrap(), Err(()));
    assert!(activity.0.lock().unwrap().discovery_started.is_none());
    assert_eq!(
        discover(Some(activity), async { Ok::<_, ()>(7) }, ()).await,
        Ok(7)
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_before_start_never_constructs_an_attempt() {
    let (stop, signal) = watch::channel(false);
    stop.send(true).unwrap();
    let activity = Workers::new().get("launches");
    spawn(
        (),
        activity.clone(),
        signal,
        |_, _, _| Box::pin(async { panic!("must not run") }),
        |_| Box::pin(async {}),
    )
    .await
    .unwrap();
    assert_eq!(activity.snapshot().state, "stopped");
}

#[tokio::test(start_paused = true)]
async fn aborting_supervisor_cancels_its_structurally_owned_children() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let marker = Dropped(dropped.clone());
    let mut children = tokio::task::JoinSet::new();
    children.spawn(async move {
        let _marker = marker;
        std::future::pending::<()>().await;
    });
    let (_stop, signal) = watch::channel(false);
    let worker = spawn(
        children,
        Workers::new().get("agents"),
        signal,
        |_, _, _| Box::pin(std::future::pending()),
        |_| Box::pin(async {}),
    );
    settle().await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    settle().await;
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_backoff_drains_retained_children_without_a_new_attempt() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let child_release = release.clone();
    let mut children = tokio::task::JoinSet::new();
    children.spawn(async move {
        child_release.notified().await;
        7
    });
    let activity = Workers::new().get("agents");
    let (stop, signal) = watch::channel(false);
    let count = attempts.clone();
    let result = completed.clone();
    let worker = spawn(
        children,
        activity.clone(),
        signal,
        move |_, _, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { panic!("scheduler failed before shutdown") })
        },
        move |children| {
            Box::pin(async move {
                while let Some(child) = children.join_next().await {
                    result.store(child.unwrap(), Ordering::SeqCst);
                }
            })
        },
    );
    settle().await;
    assert_eq!(activity.snapshot().state, "backoff");
    stop.send(true).unwrap();
    settle().await;
    assert_eq!(activity.snapshot().state, "draining");
    assert!(!worker.is_finished());
    assert_eq!(completed.load(Ordering::SeqCst), 0);
    release.notify_one();
    worker.await.unwrap();
    assert_eq!(completed.load(Ordering::SeqCst), 7);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(activity.snapshot().state, "stopped");
}

#[tokio::test(start_paused = true)]
async fn shutdown_of_suspended_loop_enters_drain_without_waiting_for_watchdog() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let child_release = release.clone();
    let mut children = tokio::task::JoinSet::new();
    children.spawn(async move {
        child_release.notified().await;
        11
    });
    let activity = Workers::new().get("agents");
    let (stop, signal) = watch::channel(false);
    let count = attempts.clone();
    let result = completed.clone();
    let worker = spawn(
        children,
        activity.clone(),
        signal,
        move |_, activity, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                activity.waiting(Duration::from_secs(1), "execution_slots");
                std::future::pending().await
            })
        },
        move |children| {
            Box::pin(async move {
                while let Some(child) = children.join_next().await {
                    result.store(child.unwrap(), Ordering::SeqCst);
                }
            })
        },
    );
    settle().await;
    let before = Instant::now();
    stop.send(true).unwrap();
    settle().await;
    assert_eq!(Instant::now(), before);
    assert_eq!(activity.snapshot().state, "draining");
    assert!(!worker.is_finished());
    release.notify_one();
    worker.await.unwrap();
    assert_eq!(Instant::now(), before);
    assert_eq!(completed.load(Ordering::SeqCst), 11);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}
